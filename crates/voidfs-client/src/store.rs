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
use unicode_normalization::UnicodeNormalization;

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
    // 4: the mount namespace (mount.rs). Identity outlives a name, including a remote deletion.
    "CREATE TABLE mount_inodes(ino INTEGER PRIMARY KEY AUTOINCREMENT, drive TEXT NOT NULL, object_id TEXT,
         attrs TEXT NOT NULL, generation INTEGER NOT NULL DEFAULT 0, sync TEXT NOT NULL DEFAULT 'saved', UNIQUE(drive, object_id));
     CREATE TABLE mount_roots(drive TEXT PRIMARY KEY, ino INTEGER NOT NULL REFERENCES mount_inodes(ino), seq INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID;
     CREATE TABLE mount_dirs(ino INTEGER PRIMARY KEY REFERENCES mount_inodes(ino), generation INTEGER NOT NULL DEFAULT 0,
         listed INTEGER NOT NULL DEFAULT 0, seq INTEGER);
     CREATE TABLE mount_names(parent INTEGER NOT NULL REFERENCES mount_inodes(ino), name TEXT NOT NULL,
         ino INTEGER NOT NULL UNIQUE REFERENCES mount_inodes(ino), PRIMARY KEY(parent, name)) WITHOUT ROWID;
     CREATE TABLE mount_overlay(parent INTEGER NOT NULL REFERENCES mount_inodes(ino), name TEXT NOT NULL,
         ino INTEGER REFERENCES mount_inodes(ino), PRIMARY KEY(parent, name)) WITHOUT ROWID;",
    // 5: indexed equivalent-name lookup and parent traversal for feed invalidation.
    "ALTER TABLE mount_names ADD COLUMN nfc TEXT NOT NULL DEFAULT '';
     ALTER TABLE mount_overlay ADD COLUMN nfc TEXT NOT NULL DEFAULT '';
     CREATE INDEX mount_names_nfc ON mount_names(parent, nfc);
     CREATE INDEX mount_overlay_nfc ON mount_overlay(parent, nfc);
     CREATE INDEX mount_overlay_ino ON mount_overlay(ino);",
    // 6: local namespace publication lineage and complete extended-attribute snapshots.
    "ALTER TABLE mount_inodes ADD COLUMN entry_id INTEGER;
     ALTER TABLE mount_inodes ADD COLUMN remote_key TEXT;
     ALTER TABLE entries ADD COLUMN mount INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE entries ADD COLUMN mount_ino INTEGER;
     CREATE INDEX mount_pending_remote_key ON mount_inodes(drive, remote_key) WHERE sync<>'saved';
     CREATE TABLE mount_xattrs(ino INTEGER PRIMARY KEY REFERENCES mount_inodes(ino), version TEXT,
         attrs TEXT NOT NULL, dirty INTEGER NOT NULL DEFAULT 0);",
    // 7: acknowledged mount bytes and their immutable remote base.
    "CREATE TABLE mount_staged(ino INTEGER PRIMARY KEY REFERENCES mount_inodes(ino), path TEXT NOT NULL, record TEXT NOT NULL);",
    // 8: remembered mounts follow stable drive identity across alias reuse.
    "ALTER TABLE mounts ADD COLUMN drive_id TEXT;",
];

pub(crate) struct MountLease { write: bool }

pub struct Store {
    dir: PathBuf,
    conn: Mutex<Connection>,
    normal: Mutex<Connection>,
    access: Mutex<()>,
    staging: Mutex<()>,
    _lock: File,
    mount_writers: Mutex<std::collections::HashMap<String, Vec<std::sync::Weak<MountLease>>>>,
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
        conn.pragma_update(None, "fullfsync", true)?;
        conn.pragma_update(None, "checkpoint_fullfsync", true)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let version = usize::try_from(version).unwrap_or(usize::MAX);
        if version > MIGRATIONS.len() {
            return Err(Error::Invalid(format!("{} was written by a newer voidfs (schema {version})", dir.display())));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            if i == 4 {
                for table in ["mount_names", "mount_overlay"] {
                    let mut q = tx.prepare(&format!("SELECT parent, name FROM {table}"))?;
                    let rows = q.query_map([], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    drop(q);
                    for (parent, name) in rows {
                        let nfc: String = name.nfc().collect();
                        tx.execute(&format!("UPDATE {table} SET nfc=?3 WHERE parent=?1 AND name=?2"), rusqlite::params![parent, name, nfc])?;
                    }
                }
            }
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
            tx.commit()?;
        }
        let normal = Connection::open(dir.join("state.sqlite"))?;
        normal.pragma_update(None, "synchronous", "NORMAL")?;
        normal.pragma_update(None, "fullfsync", true)?;
        normal.pragma_update(None, "checkpoint_fullfsync", true)?;
        normal.pragma_update(None, "foreign_keys", true)?;
        Ok(Store { dir: dir.to_owned(), conn: Mutex::new(conn), normal: Mutex::new(normal), access: Mutex::new(()), staging: Mutex::new(()), _lock: lock, mount_writers: Mutex::new(Default::default()) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn mount_session(&self, drive: &str, write: bool) -> Option<std::sync::Arc<MountLease>> {
        let mut writers = self.mount_writers.lock().unwrap_or_else(|p| p.into_inner());
        writers.retain(|_, leases| { leases.retain(|lease| lease.strong_count() > 0); !leases.is_empty() });
        let leases = writers.entry(drive.to_owned()).or_default();
        if leases.iter().filter_map(std::sync::Weak::upgrade).any(|lease| write || lease.write) { return None; }
        let lease = std::sync::Arc::new(MountLease { write });
        leases.push(std::sync::Arc::downgrade(&lease));
        Some(lease)
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
        let _access = self.access.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        Ok(f(&mut conn)?)
    }

    pub(crate) fn with_normal<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> Result<T> {
        let _access = self.access.lock().unwrap_or_else(|p| p.into_inner());
        let mut conn = self.normal.lock().unwrap_or_else(|p| p.into_inner());
        Ok(f(&mut conn)?)
    }

    pub(crate) fn staging(&self) -> std::sync::MutexGuard<'_, ()> {
        self.staging.lock().unwrap_or_else(|p| p.into_inner())
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

    #[test]
    fn namespace_migration_preserves_step_four_state() {
        let dir = tempfile::tempdir().unwrap();
        let c = Connection::open(dir.path().join("state.sqlite")).unwrap();
        for sql in &MIGRATIONS[..3] { c.execute_batch(sql).unwrap(); }
        c.pragma_update(None, "user_version", 3).unwrap();
        c.execute("INSERT INTO meta VALUES ('id', 'old-client')", []).unwrap();
        c.execute("INSERT INTO mounts VALUES ('/mount', 'drv', 'fskit', 1, 1)", []).unwrap();
        drop(c);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.id().unwrap(), "old-client");
        let remembered = crate::mounts::remembered(&s).unwrap();
        assert_eq!(remembered.len(), 1);
        assert_eq!(remembered[0].mountpoint, "/mount");
        s.with(|c| c.execute("INSERT INTO mount_inodes(drive, attrs) VALUES ('drv', '{}')", [])).unwrap();
        let version: i64 = s.with(|c| c.pragma_query_value(None, "user_version", |r| r.get(0))).unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn indexed_names_migration_backfills_remote_and_local_unicode_spellings() {
        let dir = tempfile::tempdir().unwrap();
        let c = Connection::open(dir.path().join("state.sqlite")).unwrap();
        for sql in &MIGRATIONS[..4] { c.execute_batch(sql).unwrap(); }
        c.pragma_update(None, "user_version", 4).unwrap();
        c.execute("INSERT INTO mount_inodes(ino, drive, attrs) VALUES (1, 'drv', '{}'), (2, 'drv', '{}')", []).unwrap();
        c.execute("INSERT INTO mount_names VALUES (1, ?1, 2)", ["cafe\u{301}"]).unwrap();
        c.execute("INSERT INTO mount_overlay VALUES (1, ?1, NULL)", ["cafe\u{301}"]).unwrap();
        drop(c);
        let s = Store::open(dir.path()).unwrap();
        for table in ["mount_names", "mount_overlay"] {
            let (name, nfc): (String, String) = s.with(|c| c.query_row(&format!("SELECT name, nfc FROM {table}"), [], |r| Ok((r.get(0)?, r.get(1)?)))).unwrap();
            assert_eq!(name, "cafe\u{301}");
            assert_eq!(nfc, "café");
        }
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.with(|c| c.query_row("SELECT count(*) FROM mount_overlay WHERE ino IS NULL AND nfc='café'", [], |r| r.get::<_, u64>(0))).unwrap(), 1);
    }

    #[test]
    fn writable_namespace_migration_preserves_existing_overlay_and_queue_policy() {
        let dir = tempfile::tempdir().unwrap();
        let c = Connection::open(dir.path().join("state.sqlite")).unwrap();
        for sql in &MIGRATIONS[..5] { c.execute_batch(sql).unwrap(); }
        c.pragma_update(None, "user_version", 5).unwrap();
        c.execute("INSERT INTO mount_inodes(ino, drive, attrs, generation, sync) VALUES (1, 'drv', '{}', 7, 'pending')", []).unwrap();
        c.execute("INSERT INTO mount_overlay(parent, name, ino, nfc) VALUES (1, 'gone', NULL, 'gone')", []).unwrap();
        c.execute("INSERT INTO entries(drive, key, op, base, state, created) VALUES ('drv', 'file', 'delete', 'v:old', 'queued', 1)", []).unwrap();
        drop(c);
        let s = Store::open(dir.path()).unwrap();
        s.with(|c| {
            let inode: (u64, String, Option<i64>, Option<String>) = c.query_row("SELECT generation, sync, entry_id, remote_key FROM mount_inodes WHERE ino=1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            assert_eq!(inode, (7, "pending".into(), None, None));
            assert_eq!(c.query_row("SELECT count(*) FROM mount_overlay WHERE name='gone' AND ino IS NULL AND nfc='gone'", [], |r| r.get::<_, u64>(0))?, 1);
            let entry: (String, String, bool, Option<u64>) = c.query_row("SELECT base, state, mount, mount_ino FROM entries", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            assert_eq!(entry, ("v:old".into(), "queued".into(), false, None));
            assert_eq!(c.query_row("SELECT count(*) FROM mount_xattrs", [], |r| r.get::<_, u64>(0))?, 0);
            c.execute("INSERT INTO mount_xattrs(ino, attrs) VALUES (1, '{}')", [])?;
            Ok(())
        }).unwrap();
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.with(|c| c.query_row("SELECT dirty FROM mount_xattrs WHERE ino=1", [], |r| r.get::<_, u64>(0))).unwrap(), 0);
    }

    #[test]
    fn staged_data_migration_preserves_namespace_lineage_and_uses_a_separate_normal_connection() {
        let dir = tempfile::tempdir().unwrap();
        let c = Connection::open(dir.path().join("state.sqlite")).unwrap();
        for sql in &MIGRATIONS[..6] { c.execute_batch(sql).unwrap(); }
        c.pragma_update(None, "user_version", 6).unwrap();
        c.execute("INSERT INTO mount_inodes(ino, drive, attrs, entry_id, remote_key) VALUES (1, 'drive', '{}', 9, 'old/file')", []).unwrap();
        c.execute("INSERT INTO mount_xattrs(ino, version, attrs, dirty) VALUES (1, 'version', '{}', 1)", []).unwrap();
        drop(c);
        let s = Store::open(dir.path()).unwrap();
        s.with(|c| {
            assert_eq!(c.query_row("SELECT entry_id, remote_key FROM mount_inodes WHERE ino=1", [], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?)))?, (9, "old/file".into()));
            assert_eq!(c.query_row("SELECT version, dirty FROM mount_xattrs WHERE ino=1", [], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?, ("version".into(), 1));
            assert_eq!(c.query_row("SELECT count(*) FROM mount_staged", [], |r| r.get::<_, u64>(0))?, 0);
            assert_eq!(c.pragma_query_value(None, "synchronous", |r| r.get::<_, u64>(0))?, 2);
            Ok(())
        }).unwrap();
        s.with_normal(|c| {
            assert_eq!(c.pragma_query_value(None, "synchronous", |r| r.get::<_, u64>(0))?, 1);
            c.execute("INSERT INTO mount_staged(ino, path, record) VALUES (1, '/staged/file', '{}')", [])?;
            Ok(())
        }).unwrap();
        assert_eq!(s.with(|c| c.query_row("SELECT path FROM mount_staged WHERE ino=1", [], |r| r.get::<_, String>(0))).unwrap(), "/staged/file");
        drop(s);
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.with_normal(|c| c.query_row("SELECT count(*) FROM mount_staged", [], |r| r.get::<_, u64>(0))).unwrap(), 1);
    }
}
