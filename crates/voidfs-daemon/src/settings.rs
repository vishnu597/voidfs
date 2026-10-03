// SPDX-License-Identifier: Apache-2.0
//! The connection the daemon uses when it is given none: `<state>/daemon.json`, which only its
//! user can read. `void daemon start` writes it from the connection it was given, so that a
//! daemon launchd starts at login needs no secret in its plist.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE: &str = "daemon.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub endpoint: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl Settings {
    pub fn of(c: &voidfs_sdk::Config) -> Settings {
        Settings { endpoint: c.endpoint.clone(), region: c.region.clone(), access_key_id: c.access_key_id.clone(), secret_access_key: c.secret_access_key.clone() }
    }

    /// A client configuration with this connection.
    pub fn config(&self) -> voidfs_sdk::Config {
        voidfs_sdk::Config {
            endpoint: self.endpoint.clone(),
            region: self.region.clone(),
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            ..voidfs_sdk::Config::default()
        }
    }

    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE)
    }

    /// The settings in state directory `dir`, if there are any.
    pub fn load(dir: &Path) -> io::Result<Option<Settings>> {
        match std::fs::read(Settings::path(dir)) {
            Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", Settings::path(dir).display()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Writes them, readable by this user only, in place of what was there.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{FILE}.{}", std::process::id()));
        let mut f = std::fs::File::options().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(self).expect("JSON"))?;
        f.sync_all()?;
        std::fs::rename(&tmp, Settings::path(dir))
    }
}
