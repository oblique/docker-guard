#![recursion_limit="1024"]
#[macro_use]
extern crate error_chain;
extern crate epoll;
extern crate fs2;
extern crate httparse;
extern crate url;
extern crate regex;
#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate log;
extern crate env_logger;
#[macro_use]
extern crate clap;

use std::cmp;
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::net::TcpStream;
use std::os::unix::net::{UnixListener, UnixStream};
use std::str;
use std::sync::Arc;
use std::fs::File;

use fs2::FileExt;
use clap::{App, Arg, ArgMatches};
use url::Url;

mod config;
use config::*;

mod filters;

mod errors {
    error_chain! {
        foreign_links {
            Io(::std::io::Error) #[cfg(unix)];
            HttpParse(::httparse::Error);
            Utf8Error(::std::str::Utf8Error);
            ParseIntError(::std::num::ParseIntError);
            SerdeJson(::serde_json::Error);
            Config(::config::config::ConfigError);
        }
    }
}

use errors::*;

trait Stream: Read + Write + AsRawFd {
    fn as_read_mut(&mut self) -> &mut Read;
    fn as_write_mut(&mut self) -> &mut Write;
}

impl<T> Stream for T where T: Read + Write + AsRawFd {
    fn as_read_mut(&mut self) -> &mut Read {
        self
    }

    fn as_write_mut(&mut self) -> &mut Write {
        self
    }
}

pub enum Http<'headers, 'buf: 'headers> {
    Res(httparse::Response<'headers, 'buf>),
    Req(httparse::Request<'headers, 'buf>),
}

impl<'h, 'b> Http<'h, 'b> {
    fn res(&'h self) -> Result<&'h httparse::Response<'h, 'b>> {
        match self {
            Http::Res(res) => Ok(res),
            Http::Req(_) => Err("Not an HTTP response".into()),
        }
    }

    fn req(&'h self) -> Result<&'h httparse::Request<'h, 'b>> {
        match self {
            Http::Res(_) => Err("Not an HTTP request".into()),
            Http::Req(req) => Ok(req),
        }
    }

    fn headers(&self) -> &[httparse::Header] {
        match self {
            Http::Res(res) => &res.headers,
            Http::Req(req) => &req.headers,
        }
    }
}

fn is_http_upgraded(http_req: &Http, http_res: &Http) -> Result<bool> {
    let http_req = http_req.req()?;
    let http_res = http_res.res()?;

    Ok(find_in_headers(http_req.headers, "Connection").unwrap_or("") == "Upgrade" &&
       http_res.code.unwrap_or(0) == 101)
}

fn read_until(reader: &mut Read, buf: &mut Vec<u8>, until: &[u8]) -> io::Result<()> {
    while !buf.ends_with(until) {
        let mut b = [0; 1];
        let len = reader.read(&mut b)?;
        buf.extend_from_slice(&b[..len]);
    }

    Ok(())
}

fn parse_http<'b, 'h>(http: &'b [u8], headers: &'h mut [httparse::Header<'b>]) -> Result<Http<'h, 'b>> {
    let headers = {
        let mut res = httparse::Response::new(headers);
        if let Ok(_) = res.parse(&http) {
            return Ok(Http::Res(res));
        }
        res.headers
    };

    let mut req = httparse::Request::new(headers);
    if let Ok(_) = req.parse(&http) {
        return Ok(Http::Req(req));
    }

    Err("Failed to parse HTTP headers".into())
}

fn find_in_headers<'h>(headers: &'h [httparse::Header], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|hdr| hdr.name == name)
        .and_then(|hdr| std::str::from_utf8(hdr.value).ok())
}

fn read_http_headers<'h, 'b>(
    reader: &mut Read,
    headers: &'h mut [httparse::Header<'b>],
    hdr_buf: &'b mut Vec<u8>,
) -> Result<Http<'h, 'b>> {
    read_until(reader, hdr_buf, b"\r\n\r\n").chain_err(|| "Failed to read http header")?;
    parse_http(hdr_buf, headers)
}

fn read_http_content(reader: &mut Read, http: &Http) -> Result<Vec<u8>> {
    let mut content_buf = Vec::new();
    let headers = http.headers();

    if let Some(transfer_encoding) = find_in_headers(headers, "Transfer-Encoding") {
        if transfer_encoding == "chunked" {
            // read all chunks
            loop {
                // read chunked length
                let mut buf = Vec::new();
                read_until(reader, &mut buf, b"\r\n")?;

                let chunked_len = str::from_utf8(&buf)?;
                let chunked_len = usize::from_str_radix(&chunked_len.trim(), 16)?;

                // read chunk
                let mut read_chunked_len = 0;
                while read_chunked_len < chunked_len {
                    let mut buf = [0; 4096];
                    let read_len = cmp::min(chunked_len - read_chunked_len, buf.len());
                    let len = reader.read(&mut buf[..read_len])?;
                    content_buf.extend_from_slice(&buf[..len]);
                    read_chunked_len += len;
                }

                // read CRLF
                let mut buf = [0; 2];
                reader.read_exact(&mut buf)?;
                if buf.ne(b"\r\n") {
                    return Err("Malformed chunked encoding".into());
                }

                // stop if this was the last chunk (i.e. zero-length chunk)
                if chunked_len == 0 {
                    break;
                }
            }
        } else {
            return Err(format!("Transfer-Encoding `{}` is not supported", transfer_encoding).into());
        }
    } else if let Some(content_len) = find_in_headers(headers, "Content-Length") {
        if let Ok(mut content_len) = content_len.parse::<usize>() {
            while content_len > 0 {
                let mut buf = [0; 4096];
                let len = reader.read(&mut buf)?;
                content_buf.extend_from_slice(&buf[..len]);
                content_len -= len;
            }
        }
    }

    Ok(content_buf)
}

fn write_http_headers(writer: &mut Write, http: &Http, content_len: Option<usize>) -> Result<()> {
    match http {
        Http::Res(res) => {
            let version = res.version.unwrap_or(0);
            let code = res.code.ok_or("Invalid http response code")?;
            let reason = res.reason.unwrap_or("");
            writer.write_all(format!("HTTP/1.{} {} {}\r\n", version, code, reason).as_bytes())?;
        }
        Http::Req(req) => {
            let method = req.method.ok_or("Undefined method")?;
            let path = req.path.unwrap_or("/");
            let version = req.version.unwrap_or(0);
            writer.write_all(format!("{} {} HTTP/1.{}\r\n", method, path, version).as_bytes())?;
        }
    }

    for hdr in http.headers() {
        // strip content length and transfer encoding since they could been changed, we add them later
        if hdr.name == "Content-Length" || hdr.name == "Transfer-Encoding" {
            continue;
        }
        writer.write_all(format!("{}: ", hdr.name).as_bytes())?;
        writer.write_all(hdr.value)?;
        writer.write_all(b"\r\n")?;
    }

    match content_len {
        Some(len) => {
            if len > 0 {
                writer.write_all(format!("Content-Length: {}\r\n", len).as_bytes())?;
            }
        }
        None => writer.write_all(b"Transfer-Encoding: chunked\r\n")?,
    }

    writer.write_all(b"\r\n")?;
    Ok(())
}

fn write_http_content(writer: &mut Write, content: &[u8]) -> Result<()> {
    writer.write_all(content)?;
    Ok(())
}

fn write_http_content_chunked(writer: &mut Write, content: &[u8]) -> Result<()> {
    if content.len() > 0 {
        writer.write_all(format!("{:x}\r\n", content.len()).as_bytes())?;
        writer.write_all(content)?;
        writer.write_all(b"\r\n")?;
    }
    writer.write_all(b"0\r\n\r\n")?;
    Ok(())
}

fn forward_data(from: &mut Read, to: &mut Write) -> io::Result<usize> {
    let mut buf = [0; 1024];
    let len = from.read(&mut buf)?;
    to.write_all(&buf[..len])?;
    Ok(len)
}

fn forward_http<'h, 'b: 'h, FH, FC>(
    from: &mut Read,
    to: &mut Write,
    hdr_buf: &'b mut Vec<u8>,
    headers: &'h mut [httparse::Header<'b>],
    filter_headers: FH,
    filter_content: FC,
) -> Result<Option<Http<'h, 'b>>>
where
    FH: FnOnce(&Http<'h, 'b>) -> Result<bool>,
    FC: FnOnce(&Http<'h, 'b>, &mut Vec<u8>) -> Result<bool>,
{
    let http = read_http_headers(from, headers, hdr_buf)?;

    if !filter_headers(&http)? {
        return Ok(None);
    }

    if find_in_headers(http.headers(), "Transfer-Encoding").unwrap_or("") == "chunked" {
        // in case of `chunked` transfer encoding we forward the headers before we try
        // to receive the content.
        // we do this because the content can be available after a lot of time (even minutes),
        // however we need to inform the other end that we received the headers of request/response.
        write_http_headers(to, &http, None)?;
        let mut content_buf = read_http_content(from, &http)?;
        if !filter_content(&http, &mut content_buf)? {
            return Ok(None);
        }
        write_http_content_chunked(to, &content_buf)?;
    } else {
        let mut content_buf = read_http_content(from, &http)?;
        if !filter_content(&http, &mut content_buf)? {
            return Ok(None);
        }
        write_http_headers(to, &http, Some(content_buf.len()))?;
        write_http_content(to, &content_buf)?;
    }

    Ok(Some(http))
}

fn handle_upgraded(stream1: &mut Stream, stream2: &mut Stream) -> Result<()> {
    let epfd = epoll::create(true)?;

    let ev = epoll::Event::new(epoll::Events::EPOLLIN, stream1.as_raw_fd() as u64);
    epoll::ctl(epfd, epoll::ControlOptions::EPOLL_CTL_ADD, stream1.as_raw_fd(), ev)?;

    let ev = epoll::Event::new(epoll::Events::EPOLLIN, stream2.as_raw_fd() as u64);
    epoll::ctl(epfd, epoll::ControlOptions::EPOLL_CTL_ADD, stream2.as_raw_fd(), ev)?;

    'outer: loop {
        let mut events = [epoll::Event::new(epoll::Events::EPOLLIN, 0); 2];
        let num_of_events = epoll::wait(epfd, -1, &mut events)?;

        for ev in events[..num_of_events].iter() {
            if ev.events & epoll::Events::EPOLLHUP.bits() != 0 {
                break 'outer;
            }

            let fd = ev.data as RawFd;
            if fd == stream1.as_raw_fd() {
                if forward_data(stream1.as_read_mut(), stream2.as_write_mut())? == 0 {
                    break 'outer;
                }
            } else if fd == stream2.as_raw_fd() {
                if forward_data(stream2.as_read_mut(), stream1.as_write_mut())? == 0 {
                    break 'outer;
                }
            }
        }
    }

    Ok(())
}

fn connect_to_docker(url: &Url) -> Result<Box<Stream>> {
    match url.scheme() {
        "unix" => {
            let path = match url.path() {
                "/" => "/var/run/docker.sock",
                path => path,
            };
            Ok(Box::new(UnixStream::connect(path)?))
        }
        "tcp" => {
            let host = match url.host_str().unwrap_or("") {
                "" => "127.0.0.1",
                host => host,
            };
            Ok(Box::new(TcpStream::connect(format!("{}:{}", host, url.port().unwrap_or(2375)))?))
        }
        _ => Err("Unsupported docker host uri".into())
    }
}

fn handle_client(stream: &mut UnixStream, config: Arc<Config>) -> Result<()> {
    let mut fwd = connect_to_docker(&config.docker_host)?;
    let mut filter_fn: Option<FilterFn> = None;

    // receive request for our sock and send it to the docker sock.
    let mut hdr_buf = Vec::new();
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let http_req = forward_http(stream, fwd.as_write_mut(), &mut hdr_buf, &mut headers,
                                // check if request path is allowed and retrieve the filter
                                // function for the response content.
                                |http_req| {
                                    let req = http_req.req().chain_err(|| "HTTP request was expected")?;
                                    let method = req.method.unwrap_or("UNKNOWN");
                                    let path = req.path.unwrap_or("/");
                                    match config.match_http_path(path) {
                                        Some(func) => {
                                            filter_fn = func;
                                            info!("Allow: {} {}", method, path);
                                            Ok(true)
                                        }
                                        None => {
                                            info!("Deny:  {} {}", method, path);
                                            Ok(false)
                                        }
                                    }
                                },
                                // for now we do not support filtering of request content
                                |_, _| Ok(true))?;
    // if http_req is None, then http request was filtered out
    let http_req = match http_req {
        Some(v) => v,
        None => return Ok(()),
    };

    // receive response from docker sock and send it to our sock.
    let mut hdr_buf = Vec::new();
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let http_res = forward_http(fwd.as_read_mut(), stream, &mut hdr_buf, &mut headers,
                                // ensure that we received a response
                                |http_res| http_res
                                                .res()
                                                .map(|_| Ok(true))
                                                .chain_err(|| "HTTP response was expected")?,
                                // filter content if needed
                                |http_res, content| {
                                    match filter_fn {
                                        Some(filter_fn) => filter_fn(&config,
                                                                     http_req.req().unwrap(),
                                                                     http_res.res().unwrap(),
                                                                     content),
                                        None => Ok(true),
                                    }
                                })?;
    // if http_res is None, then http response was filtered out
    let http_res = match http_res {
        Some(v) => v,
        None => return Ok(()),
    };

    if is_http_upgraded(&http_req, &http_res)? {
        handle_upgraded(stream, &mut *fwd)?;
    }

    Ok(())
}

fn run(arg_matches: ArgMatches) -> Result<()> {
    let mut config = Arc::new(Config::from_arg_matches(arg_matches)?);

    {
        let config = Arc::make_mut(&mut config);

        // allow: /_ping
        config.allow_http_path(r"^/_ping$")?;
        // allow `docker version`
        config.allow_http_path(r"^(/v[0-9\.]+)?/version$")?;
        // allow `docker info`
        config.filter_http_path(r"^(/v[0-9\.]+)?/info$", filters::info)?;
        // allow `docker ps`:
        //  /containers/json?..
        //  /v1.37/containers/json?..
        config.filter_http_path(r"^(/v[0-9\.]+)?/containers/json(\?.*)?$", filters::list)?;
        // allow `docker inspect <id>`:
        //  /containers/ID/json?..
        //  /v1.37/containers/ID/json?..
        config.filter_http_path(r"^(/v[0-9\.]+)?/containers//?[a-zA-Z0-9][a-zA-Z0-9_\.-]+/json(\?.*)?$",
                           filters::inspect)?;
    }

    // create docker_guard_dir
    fs::create_dir_all(&config.docker_guard_dir)
        .chain_err(|| format!("Failed to create `{}` directory",
                              config.docker_guard_dir.as_path().to_string_lossy()))?;

    // allow only one instance per docker_guard_dir
    let lock_file_path = config.docker_guard_dir.join("lock").as_path().to_owned();
    let lock_file = File::create(&lock_file_path)
        .chain_err(|| format!("Failed to open `{}` file", lock_file_path.to_string_lossy()))?;
    lock_file.try_lock_exclusive().chain_err(|| "docker-guard is already running")?;

    // create docker.sock of docker-guard
    let docker_guard_sock = config.docker_guard_dir.join("docker.sock").as_path().to_owned();
    fs::remove_file(&docker_guard_sock).ok();
    let listener = UnixListener::bind(&docker_guard_sock)
        .chain_err(|| format!("Failed to create `{}` socket", docker_guard_sock.to_string_lossy()))?;

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let config = Arc::clone(&config);
                std::thread::spawn(move || {
                    if let Err(ref err) = handle_client(&mut stream, config) {
                        log_error_chain(err);
                    }
                });
            }
            Err(e) => {
                return Err(Error::from(e)).chain_err(|| "Failed to accept incoming connections")
            }
        }
    }

    Ok(())
}

fn log_error_chain(err: &Error) {
    error!("Error: {}", err);
    for err in err.iter().skip(1) {
        error!("Caused by: {}", err);
    }
}

fn main() {
    let matches = App::new(crate_name!())
        .version(crate_version!())
        .author(crate_authors!())
        .arg(Arg::with_name("verbose")
             .short("v")
             .multiple(true)
             .help("Increase verbose level. Can be used multiple times."))
        .arg(Arg::with_name("ENV_WHITELIST")
             .short("e")
             .long("env")
             .env("ENV_WHITELIST")
             .takes_value(true)
             .value_name("VAR_NAME")
             .multiple(true)
             .value_delimiter(",")
             .help("White-list an environment variable. Can be used multiple times."))
        .arg(Arg::with_name("CONFIG")
             .short("c")
             .long("config")
             .env("CONFIG")
             .takes_value(true)
             .default_value("/etc/docker-guard/config.yml")
             .help("Specify a config file"))
        .arg(Arg::with_name("DOCKER_HOST")
             .short("H")
             .long("host")
             .env("DOCKER_HOST")
             .takes_value(true)
             .default_value("unix:///var/run/docker.sock")
             .help("Docker socket to connect"))
        .get_matches();

    let log_level =
        match matches.occurrences_of("verbose") {
            0 => log::LevelFilter::Warn, // Error and Warn levels are always logged
            1 => log::LevelFilter::Info,
            2 => log::LevelFilter::Debug,
            3 => log::LevelFilter::Trace,
            _ => log::LevelFilter::max(),
        };

    env_logger::Builder::from_default_env()
        .filter_level(log_level)
        .init();

    if let Err(ref e) = run(matches) {
        log_error_chain(e);
        std::process::exit(1);
    }
}
