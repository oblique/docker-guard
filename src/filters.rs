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
    content: &mut Vec<u8>,
) -> Result<bool> {
    if res.code.unwrap_or(0) != 200 {
        return Ok(false);
    }

    let json: Value = serde_json::from_slice(&content[..])?;
    let mut new_list = Vec::new();

    if let Value::Array(ref containers) = json {
        for container in containers {
            new_list.push(json!({
                "Id": container["Id"],
                "Created": container["Created"],
                "Status": container["Status"],
            }));
        }
    }

    *content = serde_json::to_vec(&json!(new_list))?;
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

    let json: Value = serde_json::from_slice(&content[..])?;
    let mut new_env = Vec::new();

    if let Value::Array(ref envs) = json["Config"]["Env"] {
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

    let new_json = json!({
        "Id": json["Id"],
        "Name": json["Name"],
        "State": json["State"],
        "NetworkSettings": json["NetworkSettings"],
        "Config": {
            "Env": new_env,
        }
    });
        }
    }

    *content = serde_json::to_vec(&new_json)?;
    Ok(true)
}
