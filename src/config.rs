pub extern crate config;

use std::path::PathBuf;
use std::collections::HashSet;
use std::path::Path;

use httparse;
use regex::Regex;
use url::Url;
use clap::ArgMatches;

use errors::*;

#[derive(Clone)]
pub struct Config {
    pub docker_host: Url,
    pub docker_guard_dir: PathBuf,
    http_path_whitelist: Vec<(Regex, Option<FilterFn>)>,
    env_whitelist: HashSet<String>,
}

pub type FilterFn = fn(&Config, &httparse::Request, &httparse::Response, &mut Vec<u8>) -> Result<bool>;

impl Config {
    pub fn from_arg_matches(matches: ArgMatches) -> Result<Config> {
        let docker_host = matches.value_of("DOCKER_HOST").unwrap();
        let docker_host = Url::parse(docker_host).chain_err(|| format!("Invalid uri: {}", docker_host))?;

        let mut env_whitelist =
            match matches.values_of("ENV_WHITELIST") {
                Some(envs) => envs.into_iter().map(|x| x.to_owned()).collect(),
                None => HashSet::new(),
            };

        let mut settings = config::Config::new();
        settings.set_default("env_whitelist", Vec::<String>::new())?;

        if let Some(config_file) = matches.value_of("CONFIG") {
            if Path::new(config_file).is_file() {
                settings.merge(config::File::with_name(&config_file))?;
            }
        }

        env_whitelist.extend(settings
                             .get_array("env_whitelist")
                             .chain_err(|| "env_whitelist in config file must be a list, not a single value")?
                             .into_iter()
                             .filter_map(|v| v.into_str().ok()));

        Ok(Config {
            docker_host: docker_host,
            docker_guard_dir: PathBuf::from("/var/run/docker-guard"),
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
