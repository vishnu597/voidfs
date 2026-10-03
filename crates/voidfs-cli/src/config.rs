// SPDX-License-Identifier: Apache-2.0
//! Where the server is and which key signs: flags, then a key file, then the environment.

use std::path::{Path, PathBuf};

use voidfs_sdk::{Client, Config};

use crate::output::{Failure, Result};

/// The connection flags every command takes.
#[derive(clap::Args, Debug, Clone)]
pub struct Connection {
    /// The voidfs server [env: VOIDFS_ENDPOINT] [default: http://127.0.0.1:9000]
    #[arg(long, global = true, value_name = "URL")]
    pub endpoint: Option<String>,
    /// The access key's id [env: VOIDFS_ACCESS_KEY_ID]
    #[arg(long, global = true, value_name = "ID")]
    pub access_key_id: Option<String>,
    /// The access key's secret [env: VOIDFS_SECRET_ACCESS_KEY]. A command line is visible to
    /// other users of the machine: prefer the environment or --key-file
    #[arg(long, global = true, value_name = "SECRET")]
    pub secret_access_key: Option<String>,
    /// A file holding the key, as `void keys generate --format json` or `--format env` writes it
    /// [env: VOIDFS_KEY_FILE]. It takes precedence over the environment's key
    #[arg(long, global = true, value_name = "FILE")]
    pub key_file: Option<PathBuf>,
}

impl Connection {
    /// The client these flags and `env` describe.
    pub fn client(&self, env: impl Fn(&str) -> Option<String>) -> Result<Client> {
        let config = self.config(env)?.ok_or_else(|| {
            Failure::new(
                "NoCredentials",
                "no access key: set VOIDFS_ACCESS_KEY_ID and VOIDFS_SECRET_ACCESS_KEY, or pass --key-file (`void keys generate --format env` makes a key)",
            )
        })?;
        Ok(Client::new(config)?)
    }

    /// The configuration these flags and `env` describe, or `None` if they give no key.
    pub fn config(&self, env: impl Fn(&str) -> Option<String>) -> Result<Option<Config>> {
        let env = |n: &str| env(n).filter(|v| !v.is_empty());
        let file = match self.key_file.clone().or_else(|| env("VOIDFS_KEY_FILE").map(PathBuf::from)) {
            Some(p) => Some(read_key_file(&p)?),
            None => None,
        };
        let (file_id, file_secret) = file.unzip();
        let id = self.access_key_id.clone().or(file_id).or_else(|| env("VOIDFS_ACCESS_KEY_ID"));
        let secret = self.secret_access_key.clone().or(file_secret).or_else(|| env("VOIDFS_SECRET_ACCESS_KEY"));
        let (Some(access_key_id), Some(secret_access_key)) = (id, secret) else {
            return Ok(None);
        };
        let mut config = Config { access_key_id, secret_access_key, ..Config::default() };
        if let Some(e) = self.endpoint.clone().or_else(|| env("VOIDFS_ENDPOINT")) {
            config.endpoint = e;
        }
        if let Some(r) = env("VOIDFS_REGION") {
            config.region = r;
        }
        Ok(Some(config))
    }
}

/// A key file: the JSON `keys generate --format json` prints, or lines of `[export ]NAME=value`
/// naming `VOIDFS_ACCESS_KEY_ID` and `VOIDFS_SECRET_ACCESS_KEY`.
fn read_key_file(path: &Path) -> Result<(String, String)> {
    let text = std::fs::read_to_string(path).map_err(|e| Failure::local(path, e))?;
    let bad = |why: &str| Failure::invalid(format!("{}: {why}", path.display()));
    if text.trim_start().starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| bad(&format!("not a key file: {e}")))?;
        let get = |n: &str| v.get(n).and_then(|s| s.as_str()).map(str::to_owned);
        return match (get("accessKeyId"), get("secretAccessKey")) {
            (Some(i), Some(s)) => Ok((i, s)),
            _ => Err(bad("needs accessKeyId and secretAccessKey")),
        };
    }
    let (mut id, mut secret) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        let value = [('\'', '\''), ('"', '"')].iter().find_map(|(a, b)| value.strip_prefix(*a)?.strip_suffix(*b)).unwrap_or(value);
        match name.trim() {
            "VOIDFS_ACCESS_KEY_ID" => id = Some(value.to_owned()),
            "VOIDFS_SECRET_ACCESS_KEY" => secret = Some(value.to_owned()),
            _ => {}
        }
    }
    id.zip(secret).ok_or_else(|| bad("needs VOIDFS_ACCESS_KEY_ID and VOIDFS_SECRET_ACCESS_KEY"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn key_files_are_json_or_shell_lines() {
        let dir = std::env::temp_dir().join(format!("void-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let json = dir.join("k.json");
        std::fs::write(&json, r#"{ "accessKeyId": "VFJSON", "secretAccessKey": "s/1+", "scope": "read" }"#).unwrap();
        assert_eq!(read_key_file(&json).unwrap(), ("VFJSON".into(), "s/1+".into()));
        let env = dir.join("k.env");
        std::fs::write(&env, "# a key\nexport VOIDFS_ACCESS_KEY_ID=VFENV\nVOIDFS_SECRET_ACCESS_KEY='a=b'\n").unwrap();
        assert_eq!(read_key_file(&env).unwrap(), ("VFENV".into(), "a=b".into()));
        std::fs::write(&env, "VOIDFS_ACCESS_KEY_ID=VFENV\n").unwrap();
        assert_eq!(read_key_file(&env).unwrap_err().code, "InvalidArgument");
        assert_eq!(read_key_file(&dir.join("missing")).unwrap_err().code, "LocalFileError");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn flags_win_over_the_key_file_which_wins_over_the_environment() {
        let dir = std::env::temp_dir().join(format!("void-config-p-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("k.env");
        std::fs::write(&file, "VOIDFS_ACCESS_KEY_ID=VFFILE\nVOIDFS_SECRET_ACCESS_KEY=filesecret\n").unwrap();
        let env = |n: &str| match n {
            "VOIDFS_ACCESS_KEY_ID" => Some("VFENV".to_owned()),
            "VOIDFS_SECRET_ACCESS_KEY" => Some("envsecret".to_owned()),
            "VOIDFS_ENDPOINT" => Some("http://env.example:9000".to_owned()),
            _ => None,
        };
        let c = Connection { endpoint: None, access_key_id: None, secret_access_key: None, key_file: None };
        let got = c.client(env).unwrap();
        assert_eq!((got.config().access_key_id.as_str(), got.config().endpoint.as_str()), ("VFENV", "http://env.example:9000"));
        let c = Connection { key_file: Some(file.clone()), ..c };
        assert_eq!(c.client(env).unwrap().config().access_key_id, "VFFILE");
        let c = Connection { access_key_id: Some("VFFLAG".into()), endpoint: Some("http://flag.example".into()), ..c };
        let got = c.client(env).unwrap();
        assert_eq!((got.config().access_key_id.as_str(), got.config().secret_access_key.as_str(), got.config().endpoint.as_str()), ("VFFLAG", "filesecret", "http://flag.example"));
        let e = Connection { endpoint: None, access_key_id: None, secret_access_key: None, key_file: None }.client(none).unwrap_err();
        assert_eq!(e.code, "NoCredentials");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
