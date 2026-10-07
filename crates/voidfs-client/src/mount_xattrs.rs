// SPDX-License-Identifier: Apache-2.0
//! Complete xattr snapshots are pinned to the accepted inode version. Dirty maps remain
//! local until publish reconciliation, independently of namespace refresh and invalidation.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use voidfs_sdk::ReadOptions;

use super::*;
use crate::journal::{Entry, Op};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XattrMode { Set, Create, Replace }

type Xattrs = BTreeMap<String, Vec<u8>>;

struct Memo { version: Option<String>, attrs: Xattrs, dirty: bool }

fn valid_xattr_name(name: &str) -> bool { !name.is_empty() && name.len() <= 255 && !name.contains('\0') }

fn valid_xattrs(attrs: &Xattrs) -> bool {
    attrs.keys().all(|name| valid_xattr_name(name))
        && attrs.iter().try_fold(0usize, |total, (name, value)| total.checked_add(name.len())?.checked_add(value.len()))
            .is_some_and(|total| total <= voidfs_core::model::MAX_XATTR_BYTES)
}

fn memo(c: &Connection, ino: Ino) -> Result<Option<Memo>> {
    let row = c.query_row("SELECT version, attrs, dirty FROM mount_xattrs WHERE ino=?1", [ino], |r| {
        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?))
    }).optional()?;
    row.map(|(version, json, dirty)| {
        let attrs: Xattrs = serde_json::from_str(&json).map_err(|e| FsError::Io(e.to_string()))?;
        if !valid_xattrs(&attrs) { return Err(FsError::Io("invalid cached extended attributes".into())); }
        Ok(Memo { version, attrs, dirty })
    }).transpose()
}

fn save_memo(c: &Connection, ino: Ino, version: &Option<String>, attrs: &Xattrs, dirty: bool) -> Result<()> {
    c.execute("INSERT INTO mount_xattrs(ino, version, attrs, dirty) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(ino) DO UPDATE SET version=excluded.version, attrs=excluded.attrs, dirty=excluded.dirty",
        params![ino, version, serde_json::to_string(attrs).expect("JSON"), dirty])?;
    Ok(())
}

fn same_snapshot(a: &Node, b: &Node) -> bool {
    a.generation == b.generation && a.entry.object_id == b.entry.object_id
        && a.entry.version_id == b.entry.version_id && a.entry.kind == b.entry.kind
}

impl Session {
    async fn xattr_snapshot(&self, ino: Ino) -> Result<(Node, String, Xattrs)> {
        for _ in 0..4 {
            self.ancestors(ino).await?;
            let offline = self.connectivity.link() == Link::Offline;
            let state = self.db(move |c, d, root| {
                let tx = c.transaction()?;
                let state = (|| {
                    let (n, key) = mutation_node(&tx, d, root, ino, offline)?;
                    if let Some(m) = memo(&tx, ino)? && (m.dirty || m.version == n.entry.version_id && (m.version.is_some() || !n.entry.has_xattrs)) {
                        return Ok((n, key, Some(m.attrs)));
                    }
                    if !n.entry.has_xattrs {
                        let attrs = Xattrs::new();
                        save_memo(&tx, ino, &n.entry.version_id, &attrs, false)?;
                        return Ok((n, key, Some(attrs)));
                    }
                    if offline { return Err(FsError::Offline); }
                    Ok((n, key, None))
                })();
                if state.is_ok() { tx.commit()?; }
                Ok(state)
            }).await?;
            let (n, key, cached) = match state { Err(FsError::Again) => continue, other => other? };
            if let Some(attrs) = cached { return Ok((n, key, attrs)); }
            let version = match n.entry.version_id.clone().filter(|v| !v.is_empty()) {
                Some(version) => version,
                None if n.entry.kind == Kind::Folder && !n.entry.object_id.is_empty() => {
                    match self.prepare_mutation(ino).await { Err(FsError::Again) => continue, other => other? }
                    continue;
                }
                None => return Err(FsError::Io("missing extended attribute snapshot version".into())),
            };
            let remote_key = match &n.remote_key {
                Some(key) => key.clone(),
                None => match self.db(move |c, d, root| Ok(remote_path(c, d, root, ino))).await? {
                    Err(FsError::Again) => continue, other => other?,
                },
            };
            let opts = ReadOptions { version_id: Some(version), ..Default::default() };
            let remote = match self.client.attributes(&self.drive, &remote_key, opts.clone()).await {
                Err(e) if e.status() == Some(404) && remote_key != key => self.client.attributes(&self.drive, &key, opts).await,
                other => other,
            }.map_err(|e| sdk_error(&e))?;
            if remote.object_id != n.entry.object_id || remote.version_id != n.entry.version_id || remote.kind != n.entry.kind {
                return Err(FsError::Io("extended attributes returned the wrong object or version".into()));
            }
            let attrs: Xattrs = remote.xattrs.into_iter().map(|(name, encoded)| {
                STANDARD.decode(encoded).map(|value| (name, value)).map_err(|_| FsError::Io("invalid extended attribute base64".into()))
            }).collect::<Result<_>>()?;
            if !valid_xattrs(&attrs) { return Err(FsError::Io("invalid remote extended attributes".into())); }
            let old_key = key.clone();
            let accepted = self.db(move |c, d, root| {
                let tx = c.transaction()?;
                let state = (|| {
                    let (current, current_key) = mutation_node(&tx, d, root, ino, offline)?;
                    if current_key != old_key || !same_snapshot(&n, &current) { return Err(FsError::Again); }
                    // A concurrently accepted local mutation owns its full map.
                    if let Some(m) = memo(&tx, ino)? && m.dirty { return Err(FsError::Again); }
                    save_memo(&tx, ino, &current.entry.version_id, &attrs, false)?;
                    Ok((current, current_key, attrs))
                })();
                if state.is_ok() { tx.commit()?; }
                Ok(state)
            }).await?;
            match accepted { Err(FsError::Again) => continue, other => return other }
        }
        Err(FsError::Again)
    }

    /// Returns raw bytes, including an empty value. A missing name is ENOATTR/ENODATA.
    pub async fn getxattr(&self, ino: Ino, name: &str) -> Result<Vec<u8>> {
        if !valid_xattr_name(name) { return Err(FsError::InvalidName); }
        let (_, _, mut attrs) = self.xattr_snapshot(ino).await?;
        attrs.remove(name).ok_or(FsError::NoAttr)
    }

    /// Sorted byte-exact names from one complete local or pinned remote xattr snapshot.
    pub async fn listxattr(&self, ino: Ino) -> Result<Vec<String>> {
        Ok(self.xattr_snapshot(ino).await?.2.into_keys().collect())
    }

    /// Updates the durable local map and its guarded journal entry in one transaction.
    pub async fn setxattr(&self, ino: Ino, name: &str, value: &[u8], mode: XattrMode) -> Result<()> {
        if !valid_xattr_name(name) { return Err(FsError::InvalidName); }
        if name.len().saturating_add(value.len()) > voidfs_core::model::MAX_XATTR_BYTES { return Err(FsError::TooLarge); }
        self.change_xattr(ino, name.to_owned(), Some((value.to_vec(), mode))).await
    }

    pub async fn removexattr(&self, ino: Ino, name: &str) -> Result<()> {
        if !valid_xattr_name(name) { return Err(FsError::InvalidName); }
        self.change_xattr(ino, name.to_owned(), None).await
    }

    async fn change_xattr(&self, ino: Ino, name: String, value: Option<(Vec<u8>, XattrMode)>) -> Result<()> {
        if self.queue.is_none() { return Err(FsError::ReadOnly); }
        if ino == self.root { return Err(FsError::Unsupported); }
        for _ in 0..4 {
            match self.prepare_mutation(ino).await { Err(FsError::Again) => continue, other => other? }
            let (snapshot, key, _) = match self.xattr_snapshot(ino).await {
                Err(FsError::Again) => continue, other => other?,
            };
            let drive = self.drive.clone();
            let root = self.root;
            let offline = self.connectivity.link() == Link::Offline;
            let name = name.clone();
            let value = value.clone();
            let result = self.local_transaction(move |tx| {
                let (mut current, current_key) = mutation_node(tx, &drive, root, ino, offline)?;
                if current_key != key || !same_snapshot(&snapshot, &current) { return Err(FsError::Again); }
                let m = memo(tx, ino)?.ok_or_else(|| FsError::Io("missing complete extended attribute snapshot".into()))?;
                if !m.dirty && m.version != current.entry.version_id { return Err(FsError::Again); }
                let mut attrs = m.attrs;
                let exists = attrs.contains_key(&name);
                let mut entry = Entry::new(&drive, &current_key, Op::Attrs, mutation_base(&current)?);
                match value {
                    Some((bytes, mode)) => {
                        if mode == XattrMode::Create && exists { return Err(FsError::Exists); }
                        if mode == XattrMode::Replace && !exists { return Err(FsError::NoAttr); }
                        entry.attrs.xattrs.insert(name.clone(), bytes.clone());
                        attrs.insert(name, bytes);
                    }
                    None => {
                        if !exists { return Err(FsError::NoAttr); }
                        attrs.remove(&name);
                        entry.attrs.remove_xattrs.push(name);
                    }
                }
                if !valid_xattrs(&attrs) { return Err(FsError::TooLarge); }
                current.entry.has_xattrs = !attrs.is_empty();
                current.generation += 1;
                current.sync = Sync::Pending;
                save_node(tx, &current)?;
                save_memo(tx, ino, &current.entry.version_id, &attrs, true)?;
                entry.mount_ino = Some(ino);
                let change = notify::attribute(tx, &drive, root, ino)?;
                Ok((((), change), vec![entry]))
            }).await;
            match result { Err(FsError::Again) => continue, other => return other }
        }
        Err(FsError::Again)
    }
}
