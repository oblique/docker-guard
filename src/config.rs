use std::path::PathBuf;
use std::collections::HashSet;
use httparse;
use regex::Regex;

use errors::*;

#[derive(Clone)]
pub struct Config {
    pub docker_sock_path: PathBuf,
    pub docker_guard_path: PathBuf,
    allowed_paths: Vec<(Regex, Option<FilterFn>)>,
    allowed_env_vars: HashSet<String>,
}

pub type FilterFn = fn(&Config, &httparse::Request, &httparse::Response, &mut Vec<u8>) -> Result<bool>;

impl Config {
    pub fn new() -> Config {
        Config {
            docker_sock_path: PathBuf::from("/var/run/docker.sock"),
            docker_guard_path: PathBuf::from("/var/run/docker-guard/docker.sock"),
            allowed_paths: Vec::new(),
            allowed_env_vars: HashSet::new(),
        }
    }

    pub fn allow_path(&mut self, str_re: &str) -> Result<()> {
        let re = Regex::new(str_re).chain_err(|| format!("Invalid regex: {}", str_re))?;
        self.allowed_paths.push((re, None));
        Ok(())
    }

    pub fn filter_path(&mut self, str_re: &str, filter_content: FilterFn) -> Result<()> {
        let re = Regex::new(str_re).chain_err(|| format!("Invalid regex: {}", str_re))?;
        self.allowed_paths.push((re, Some(filter_content)));
        Ok(())
    }

    /// Returns `None` if path is not allowed, otherwise `Some(Option<FilterFn>)`.
    /// If `Option<FilterFn>` is `None` then no extra filtering is needed and
    /// content must be forwarded.
    pub fn match_path(&self, path: &str) -> Option<Option<FilterFn>> {
        for (re, filter_fn) in &self.allowed_paths {
            if re.is_match(path) {
                return Some(*filter_fn);
            }
        }
        None
    }

    pub fn allow_env_var(&mut self, env_var_name: &str) {
        self.allowed_env_vars.insert(env_var_name.to_string());
    }

    pub fn valid_env_var(&self, env_var_name: &str) -> bool {
        self.allowed_env_vars.contains(env_var_name)
    }
}
