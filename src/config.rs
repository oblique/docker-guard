pub extern crate config;

use std::path::PathBuf;
use std::collections::HashSet;
use std::iter::FromIterator;
use std::env;
use std::path::Path;

use httparse;
use regex::Regex;

use errors::*;

#[derive(Clone)]
pub struct Config {
    pub docker_sock_path: PathBuf,
    pub docker_guard_path: PathBuf,
    http_path_whitelist: Vec<(Regex, Option<FilterFn>)>,
    env_whitelist: HashSet<String>,
}

pub type FilterFn = fn(&Config, &httparse::Request, &httparse::Response, &mut Vec<u8>) -> Result<bool>;

impl Config {
    fn config_file_path() -> Option<String> {
        match env::var("CONFIG") {
            Ok(path) => Some(path),
            Err(_) => {
                let path = "/etc/docker-guard/config.yml".to_string();
                match Path::new(&path).exists() {
                    true => Some(path),
                    false => None,
                }
            }
        }
    }

    pub fn new() -> Result<Config> {
        let mut settings = config::Config::new();

        settings
            .set_default("version", "1")?
            .set_default("docker_sock_path", "/var/run/docker.sock")?
            .set_default("docker_guard_path", "/var/run/docker-guard/docker.sock")?
            .set_default("env_whitelist", Vec::<String>::new())?
            .merge(config::Environment::with_prefix("APP"))?;

        if let Some(path) = Config::config_file_path() {
            settings.merge(config::File::with_name(&path))?;
        }

        let docker_sock_path = settings.get_str("docker_sock_path")?.into();
        let docker_guard_path = settings.get_str("docker_guard_path")?.into();
        let env_whitelist = HashSet::from_iter(settings
                                               .get_array("env_whitelist")
                                               .chain_err(|| "Expecting a list for env_whitelist")?
                                               .into_iter()
                                               .filter_map(|v| v.into_str().ok()));

        Ok(Config {
            docker_sock_path: docker_sock_path,
            docker_guard_path: docker_guard_path,
            http_path_whitelist: Vec::new(),
            env_whitelist: env_whitelist,
        })
    }

    pub fn allow_http_path(&mut self, str_re: &str) -> Result<()> {
        let re = Regex::new(str_re).chain_err(|| format!("Invalid regex: {}", str_re))?;
        self.http_path_whitelist.push((re, None));
        Ok(())
    }

    pub fn filter_http_path(&mut self, str_re: &str, filter_content: FilterFn) -> Result<()> {
        let re = Regex::new(str_re).chain_err(|| format!("Invalid regex: {}", str_re))?;
        self.http_path_whitelist.push((re, Some(filter_content)));
        Ok(())
    }

    /// Returns `None` if path is not allowed, otherwise `Some(Option<FilterFn>)`.
    /// If `Option<FilterFn>` is `None` then no extra filtering is needed and
    /// content must be forwarded.
    pub fn match_http_path(&self, path: &str) -> Option<Option<FilterFn>> {
        for (re, filter_fn) in &self.http_path_whitelist {
            if re.is_match(path) {
                return Some(*filter_fn);
            }
        }
        None
    }

    pub fn whitelisted_env(&self, env_var_name: &str) -> bool {
        self.env_whitelist.contains(env_var_name)
    }
}
