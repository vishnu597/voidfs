// SPDX-License-Identifier: Apache-2.0
//! The remembered mounts (step 4, item 4): the drives the daemon mounts again when it starts, as
//! SpaceFS's daemon restores its remembered drives. Mounting itself is the adapters' (step 5).

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::store::Store;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Remembered {
    /// Absolute.
    pub mountpoint: String,
    /// The drive's alias.
    pub drive: String,
    /// The stable identity, absent in older remembered mounts and provisional offline mounts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_id: Option<String>,
    pub adapter: String,
    pub read_only: bool,
}

/// Every remembered mount, by mountpoint.
pub fn remembered(store: &Store) -> Result<Vec<Remembered>> {
    store.with(|c| {
        c.prepare("SELECT mountpoint, drive, adapter, read_only, drive_id FROM mounts ORDER BY mountpoint")?
            .query_map([], |r| Ok(Remembered { mountpoint: r.get(0)?, drive: r.get(1)?, adapter: r.get(2)?, read_only: r.get(3)?, drive_id: r.get(4)? }))?
            .collect()
    })
}

/// Remembers a mount, in place of one at the same mountpoint.
pub fn remember(store: &Store, m: &Remembered) -> Result<()> {
    store.with(|c| {
        c.execute(
            "INSERT OR REPLACE INTO mounts(mountpoint, drive, adapter, read_only, created, drive_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![m.mountpoint, m.drive, m.adapter, m.read_only, crate::journal::now_ms(), m.drive_id],
        )
        .map(drop)
    })
}

/// Forgets the mount at `mountpoint`; whether there was one.
pub fn forget(store: &Store, mountpoint: &str) -> Result<bool> {
    store.with(|c| c.execute("DELETE FROM mounts WHERE mountpoint = ?1", [mountpoint]).map(|n| n > 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounts_are_remembered_across_opens_until_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        let m = Remembered { mountpoint: "/Users/a/voidfs/footage".into(), drive: "footage".into(), drive_id: Some("stable-footage".into()), adapter: "fskit".into(), read_only: false };
        remember(&s, &m).unwrap();
        remember(&s, &Remembered { read_only: true, ..m.clone() }).unwrap();
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(remembered(&s).unwrap(), [Remembered { read_only: true, ..m.clone() }], "one per mountpoint, the last kept");
        assert!(forget(&s, &m.mountpoint).unwrap());
        assert!(!forget(&s, &m.mountpoint).unwrap());
        assert!(remembered(&s).unwrap().is_empty());
    }
}
