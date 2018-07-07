use regex::Regex;
use serde_json;
use serde_json::Value;
use httparse;

use config::Config;
use errors::*;

/// Filter for `docker ps`
pub fn list(
    _config: &Config,
    _req: &httparse::Request,
    res: &httparse::Response,
    _content: &mut Vec<u8>,
) -> Result<bool> {
    if res.code.unwrap_or(0) != 200 {
        return Ok(false);
    }
    Ok(true)
}

/// Filter for `docker inspect <id>`
pub fn inspect(
    config: &Config,
    _req: &httparse::Request,
    res: &httparse::Response,
    content: &mut Vec<u8>,
) -> Result<bool> {
    if res.code.unwrap_or(0) != 200 {
        return Ok(false);
    }

    let mut json: Value = serde_json::from_slice(&content[..])?;
    let mut new_env = Vec::new();

    match json["Config"]["Env"] {
        Value::Array(ref envs) => {
            let re = Regex::new("^([^=]+)=(.+)$").unwrap();
            for env in envs {
                if let Value::String(env) = env {
                    if let Some(caps) = re.captures(env) {
                        let name = caps.get(1).unwrap().as_str();
                        if config.valid_env_var(name) {
                            new_env.push(json!(env));
                        }
                    }
                }
            }
        }
        _ => { }
    }

    if new_env.is_empty() {
        // remove Config.Env if exists
        if let Value::Object(ref mut obj) = json["Config"] {
            obj.remove("Env");
        }
    } else {
        // set the new environment variables
        json["Config"]["Env"] = json!(new_env);
    }

    content.clear();
    content.extend_from_slice(serde_json::to_string(&json)?.as_bytes());
    Ok(true)
}
