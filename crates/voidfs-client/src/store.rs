// SPDX-License-Identifier: Apache-2.0
//! The per-user state: one SQLite database, `state.sqlite`, in the state directory, with the
//! cache's index (and, in later items, the write journal and the upload queue). Content lives in
//! files beside it.
//!
//! One process owns a state directory at a time: [`Store::open`] takes an exclusive lock on
//! `<dir>/lock` and holds it while the store lives.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Each entry takes the schema from the version before it to the next.
const MIGRATIONS: &[&str] = &[
    // 1: the cache's index (cache.rs).
    "CREATE TABLE cache_blocks(
         drive TEXT NOT NULL, etag TEXT NOT NULL, block INTEGER NOT NULL,
         len INTEGER NOT NULL, sums BLOB NOT NULL, last_used INTEGER NOT NULL,
         PRIMARY KEY(drive, etag, block)) WITHOUT ROWID;
     CREATE TABLE cache_pins(drive TEXT NOT NULL, etag TEXT NOT NULL, PRIMARY KEY(drive, etag)) WITHOUT ROWID;",
    // 2: the write journal and the upload queue (journal.rs, queue.rs).
    "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
     CREATE TABLE batches(id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT NOT NULL, created INTEGER NOT NULL, paused INTEGER NOT NULL DEFAULT 0);
     CREATE TABLE entries(
         id INTEGER PRIMARY KEY AUTOINCREMENT, drive TEXT NOT NULL, key TEXT NOT NULL, op TEXT NOT NULL, base TEXT NOT NULL,
         source TEXT, staged INTEGER NOT NULL DEFAULT 0, stamp TEXT, pos INTEGER NOT NULL DEFAULT 0, length INTEGER NOT NULL DEFAULT 0,
         to_key TEXT, overwrite INTEGER NOT NULL DEFAULT 0, attrs TEXT, batch INTEGER REFERENCES batches(id),
         state TEXT NOT NULL, paused INTEGER NOT NULL DEFAULT 0, sent INTEGER NOT NULL DEFAULT 0, size INTEGER NOT NULL DEFAULT 0,
         version TEXT, conflict TEXT, error TEXT, upload_id TEXT, created INTEGER NOT NULL);
     CREATE INDEX entries_state ON entries(state, id);
     CREATE TABLE parts(entry INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE, number INTEGER NOT NULL,
         etag TEXT NOT NULL, size INTEGER NOT NULL, PRIMARY KEY(entry, number)) WITHOUT ROWID;
     CREATE TABLE paused_drives(drive TEXT PRIMARY KEY) WITHOUT ROWID;",
    // 3: the remembered mounts (mounts.rs).
    "CREATE TABLE mounts(mountpoint TEXT PRIMARY KEY, drive TEXT NOT NULL, adapter TEXT NOT NULL,
         read_only INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL) WITHOUT ROWID;",
];

pub struct Store {
    dir: PathBuf,
    conn: Mutex<Connection>,
    _lock: File,
}

impl Store {
    /// Opens (creating) the state in `dir`.
    pub fn open(dir: &Path) -> Result<Store> {
        std::fs::create_dir_all(dir)?;
        let lock = File::options().create(true).truncate(false).write(true).open(dir.join("lock"))?;
        if lock.try_lock().is_err() {
            return Err(Error::Locked(dir.display().to_string()));
        }
        let mut conn = Connection::open(dir.join("state.sqlite"))?;
        // FULL: the journal's entries are durable once a transaction returns (item 3's design).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let version = usize::try_from(version).unwrap_or(usize::MAX);
        if version > MIGRATIONS.len() {
            return Err(Error::Invalid(format!("{} was written by a newer voidfs (schema {version})", dir.display())));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
            tx.commit()?;
        }
        Ok(Store { dir: dir.to_owned(), conn: Mutex::new(conn), _lock: lock })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// This state's own id, made once: it names what this client wrote, so that it can tell its
    /// own writes from others' after losing an answer.
    pub fn id(&self) -> Result<String> {
        if let Some(id) = self.meta("id")? {
            return Ok(id);
        }
        let mut raw = [0u8; 12];
        use std::io::Read;
        File::open("/dev/urandom")?.read_exact(&mut raw)?;
        let id = hex::encode(raw);
        self.set_meta("id", &id)?;
        Ok(id)
    }

    pub(crate) fn meta(&self, key: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.with(|c| c.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional())
    }

    pub(crate) fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| c.execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", [key, value]).map(drop))
    }

    /// Runs `f` on the connection. It blocks: async callers run it on a blocking thread.
    pub(crate) fn with<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        Ok(f(&mut conn)?)
    }
}

/// The platform's place for per-user state: `~/Library/Application Support/voidfs` on macOS,
/// `$XDG_STATE_HOME/voidfs` (or `~/.local/state/voidfs`) elsewhere. `VOIDFS_STATE_DIR` overrides
/// both.
pub fn default_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("VOIDFS_STATE_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library/Application Support/voidfs"));
    }
    match std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        Some(d) => Some(PathBuf::from(d).join("voidfs")),
        None => home.map(|h| h.join(".local/state/voidfs")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_process_at_a_time_and_migrations_run_once() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(dir.path()).unwrap();
        assert!(matches!(Store::open(dir.path()), Err(Error::Locked(_))), "a second open is refused while the first lives");
        let v: i64 = s.with(|c| c.pragma_query_value(None, "user_version", |r| r.get(0))).unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        s.with(|c| c.execute("INSERT INTO cache_pins VALUES ('d', 'e')", [])).unwrap();
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        let n: i64 = s.with(|c| c.query_row("SELECT count(*) FROM cache_pins", [], |r| r.get(0))).unwrap();
        assert_eq!(n, 1, "reopening keeps the rows and runs no migration again");
        let mode: String = s.with(|c| c.pragma_query_value(None, "journal_mode", |r| r.get(0))).unwrap();
        assert_eq!(mode, "wal");
    }
}
