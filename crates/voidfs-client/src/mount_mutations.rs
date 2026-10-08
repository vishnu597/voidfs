// SPDX-License-Identifier: Apache-2.0
//! Namespace changes and their publication entries share one durable transaction.

use super::*;
use crate::journal::{Attrs, Entry, Op, StoredBase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameMode { Replace, Exclusive, Swap }

fn local_name(name: &str) -> Result<String> {
    if !valid_name(name) { return Err(FsError::InvalidName); }
    let name: String = name.nfc().collect();
    if !valid_name(&name) { return Err(FsError::InvalidName); }
    Ok(name)
}

fn child(c: &Connection, drive: &str, parent: Ino, name: &str) -> Result<Option<(String, Node)>> {
    let nfc: String = name.nfc().collect();
    let candidates = equivalent_names(c, parent, &nfc)?;
    if candidates.len() > 1 { return Err(FsError::Ambiguous); }
    let Some(ino) = candidates.first() else { return Ok(None); };
    let n = node(c, drive, *ino)?.ok_or(FsError::Stale)?;
    let (p, name) = super::parent(c, *ino)?.ok_or(FsError::Stale)?;
    if p != parent { return Err(FsError::Again); }
    Ok(Some((name, n)))
}

fn directory(c: &Connection, drive: &str, root: Ino, ino: Ino, offline: bool) -> Result<String> {
    let (n, key) = mutation_node(c, drive, root, ino, offline)?;
    if n.entry.kind != Kind::Folder { return Err(FsError::NotDir); }
    mutation_listing(c, drive, root, ino, offline)?;
    Ok(key)
}

fn key(parent: &str, name: &str, folder: bool) -> Result<String> {
    let key = format!("{parent}{name}{}", if folder { "/" } else { "" });
    voidfs_core::names::Key::parse(&key).map_err(|_| FsError::InvalidName)?;
    Ok(key)
}

fn overlay(c: &Connection, parent: Ino, name: &str, ino: Option<Ino>, owner: Ino) -> Result<()> {
    let nfc: String = name.nfc().collect();
    c.execute("INSERT INTO mount_overlay(parent, name, ino, nfc) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(parent, name) DO UPDATE SET ino=excluded.ino, nfc=excluded.nfc", params![parent, name, ino, nfc])?;
    c.execute("INSERT INTO mount_overlay_publications(parent, name, ino) VALUES (?1, ?2, ?3)
        ON CONFLICT(parent, name) DO UPDATE SET ino=excluded.ino, entry_id=NULL", params![parent, name, owner])?;
    Ok(())
}

fn changed(c: &Connection, dir: Ino) -> Result<()> {
    c.execute("UPDATE mount_dirs SET generation=generation+1 WHERE ino=?1", [dir])?;
    c.execute("UPDATE mount_inodes SET generation=generation+1 WHERE ino=?1", [dir])?;
    Ok(())
}

fn entry(drive: &str, key: &str, op: Op, n: &Node) -> Result<Entry> {
    let mut entry = Entry::new(drive, key, op, mutation_base(n)?);
    entry.mount_ino = Some(n.ino);
    Ok(entry)
}

fn invalidation(key: String, folder: bool) -> Invalidation {
    if folder { Invalidation::Subtree(key) } else { Invalidation::Object(key) }
}

impl Session {
    async fn prepare_parent(&self, ino: Ino) -> Result<()> {
        self.ancestors(ino).await?;
        self.refresh_dir(ino).await
    }

    /// Creates an empty local file exclusively, with an NFC name.
    pub async fn create(&self, parent: Ino, name: &str, mode: u32) -> Result<Attr> {
        self.create_node(parent, name, mode, Kind::File).await
    }

    pub async fn mkdir(&self, parent: Ino, name: &str, mode: u32) -> Result<Attr> {
        self.create_node(parent, name, mode, Kind::Folder).await
    }

    async fn create_node(&self, parent: Ino, name: &str, mode: u32, kind: Kind) -> Result<Attr> {
        if self.queue.is_none() { return Err(FsError::ReadOnly); }
        let name = local_name(name)?;
        if mode > 0o7777 { return Err(FsError::InvalidArgument); }
        for _ in 0..4 {
            self.prepare_parent(parent).await?;
            let drive = self.drive.clone();
            let root = self.root;
            let name = name.clone();
            let offline = self.connectivity.link() == Link::Offline;
            let result = self.local_transaction(move |tx| {
                let parent_key = directory(tx, &drive, root, parent, offline)?;
                if child(tx, &drive, parent, &name)?.is_some() { return Err(FsError::Exists); }
                let key = key(&parent_key, &name, kind == Kind::Folder)?;
                let attrs = FolderEntry { name: if kind == Kind::Folder { format!("{name}/") } else { name.clone() }, kind,
                    object_id: String::new(), version_id: None, size: (kind == Kind::File).then_some(0), etag: None,
                    mtime: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)), mode: Some(format!("{mode:04o}")), has_xattrs: false, target: None };
                tx.execute("INSERT INTO mount_inodes(drive, attrs, generation, sync) VALUES (?1, ?2, 1, 'pending')", params![drive, serde_json::to_string(&attrs).expect("JSON")])?;
                let ino = tx.last_insert_rowid() as Ino;
                if kind == Kind::Folder { tx.execute("INSERT INTO mount_dirs(ino, listed) VALUES (?1, 1)", [ino])?; }
                tx.execute("INSERT INTO mount_xattrs(ino, attrs, dirty) VALUES (?1, '{}', 1)", [ino])?;
                overlay(tx, parent, &name, Some(ino), ino)?;
                changed(tx, parent)?;
                publication::inherit(tx,&drive,root,ino)?;
                let mut entry = Entry::new(&drive, &key, if kind == Kind::Folder { Op::Folder } else { Op::Put }, StoredBase::Absent);
                entry.mount_ino = Some(ino);
                entry.attrs = Attrs { mode: Some(mode), mtime: attrs.mtime.clone(), ..Default::default() };
                let attr = node(tx, &drive, ino)?.ok_or(FsError::Stale)?.attr()?;
                let change = LocalChange { invalidations: vec![invalidation(key, kind == Kind::Folder)], inodes: vec![parent, ino], namespace: true };
                Ok(((attr, change), vec![entry]))
            }).await;
            match result { Err(FsError::Again) => continue, other => return other }
        }
        Err(FsError::Again)
    }

    pub async fn unlink(&self, parent: Ino, name: &str) -> Result<()> {
        self.remove_node(parent, name, false).await
    }

    /// Removes an empty merged directory. The queue orders it after earlier child removals.
    pub async fn rmdir(&self, parent: Ino, name: &str) -> Result<()> {
        self.remove_node(parent, name, true).await
    }

    async fn remove_node(&self, parent: Ino, name: &str, folder: bool) -> Result<()> {
        if self.queue.is_none() { return Err(FsError::ReadOnly); }
        let name = local_name(name)?;
        for _ in 0..4 {
            self.prepare_parent(parent).await?;
            let query_name = name.clone();
            let candidate = self.db(move |c, d, _| Ok(child(c, d, parent, &query_name))).await??.ok_or(FsError::NotFound)?.1;
            if folder && candidate.entry.kind != Kind::Folder { return Err(FsError::NotDir); }
            if !folder && candidate.entry.kind == Kind::Folder { return Err(FsError::IsDir); }
            self.prepare_mutation(candidate.ino).await?;
            if folder { self.refresh_dir(candidate.ino).await?; }
            let drive = self.drive.clone();
            let root = self.root;
            let name = name.clone();
            let offline = self.connectivity.link() == Link::Offline;
            let result = self.local_transaction(move |tx| {
                let parent_key = directory(tx, &drive, root, parent, offline)?;
                let (actual, mut n) = child(tx, &drive, parent, &name)?.ok_or(FsError::NotFound)?;
                if n.ino != candidate.ino { return Err(FsError::Again); }
                if folder {
                    if n.entry.kind != Kind::Folder { return Err(FsError::NotDir); }
                    directory(tx, &drive, root, n.ino, offline)?;
                    if !names(tx, n.ino)?.is_empty() { return Err(FsError::NotEmpty); }
                } else if n.entry.kind == Kind::Folder { return Err(FsError::IsDir); }
                let key = key(&parent_key, &actual, folder)?;
                let entry = entry(&drive, &key, Op::Delete, &n)?;
                overlay(tx, parent, &actual, None, n.ino)?;
                n.sync = publication::pending(tx, n.ino)?;
                n.generation += 1;
                save_node(tx, &n)?;
                changed(tx, parent)?;
                let change = LocalChange { invalidations: vec![invalidation(key, folder)], inodes: vec![parent, n.ino], namespace: true };
                Ok((((), change), vec![entry]))
            }).await;
            match result { Err(FsError::Again) => continue, other => return other }
        }
        Err(FsError::Again)
    }

    /// Hard links are unsupported (`Capabilities::hard_links`).
    pub async fn link(&self, _ino: Ino, _parent: Ino, _name: &str) -> Result<Attr> {
        Err(if self.queue.is_none() { FsError::ReadOnly } else { FsError::Unsupported })
    }

    /// Cloning is unsupported (`Capabilities::clone`).
    pub async fn clone_file(&self, _ino: Ino, _parent: Ino, _name: &str) -> Result<Attr> {
        Err(if self.queue.is_none() { FsError::ReadOnly } else { FsError::Unsupported })
    }

    /// Replaces atomically in the local view. Cloud replacement deletes the destination under
    /// its guard, then renames exclusively because the wire operation guards only the source.
    pub async fn rename(&self, from_parent: Ino, from_name: &str, to_parent: Ino, to_name: &str, how: RenameMode) -> Result<()> {
        if self.queue.is_none() { return Err(FsError::ReadOnly); }
        if how == RenameMode::Swap { return Err(FsError::Unsupported); }
        let from_name = local_name(from_name)?;
        let to_name = local_name(to_name)?;
        for _ in 0..4 {
            self.prepare_parent(from_parent).await?;
            self.prepare_parent(to_parent).await?;
            let source_name = from_name.clone();
            let source = self.db(move |c, d, _| Ok(child(c, d, from_parent, &source_name))).await??.ok_or(FsError::NotFound)?.1;
            self.prepare_mutation(source.ino).await?;
            let target_name = to_name.clone();
            let target = self.db(move |c, d, _| Ok(child(c, d, to_parent, &target_name))).await??;
            if let Some((_, target)) = &target {
                if target.ino == source.ino { return Ok(()); }
                if how == RenameMode::Exclusive { return Err(FsError::Exists); }
                if source.entry.kind == Kind::Folder && target.entry.kind != Kind::Folder { return Err(FsError::NotDir); }
                if source.entry.kind != Kind::Folder && target.entry.kind == Kind::Folder { return Err(FsError::IsDir); }
                self.prepare_mutation(target.ino).await?;
                if target.entry.kind == Kind::Folder { self.refresh_dir(target.ino).await?; }
            }
            let drive = self.drive.clone();
            let root = self.root;
            let from_name = from_name.clone();
            let to_name = to_name.clone();
            let offline = self.connectivity.link() == Link::Offline;
            let result = self.local_transaction(move |tx| {
                let from_key = directory(tx, &drive, root, from_parent, offline)?;
                let to_key = directory(tx, &drive, root, to_parent, offline)?;
                let (actual, mut n) = child(tx, &drive, from_parent, &from_name)?.ok_or(FsError::NotFound)?;
                if n.ino != source.ino { return Err(FsError::Again); }
                let destination = child(tx, &drive, to_parent, &to_name)?;
                if destination.as_ref().map(|(_, n)| n.ino) != target.as_ref().map(|(_, n)| n.ino) { return Err(FsError::Again); }
                if destination.as_ref().is_some_and(|(_, other)| other.ino == n.ino) {
                    return Ok((((), LocalChange { invalidations: Vec::new(), inodes: Vec::new(), namespace: true }), vec![]));
                }
                if n.entry.kind == Kind::Folder && chain(tx, &drive, root, to_parent)?.ok_or(FsError::Stale)?.iter().any(|(at, _)| *at == n.ino) { return Err(FsError::InvalidArgument); }
                let from = key(&from_key, &actual, n.entry.kind == Kind::Folder)?;
                let to = key(&to_key, &to_name, n.entry.kind == Kind::Folder)?;
                let mut entries = Vec::new();
                let mut inodes = vec![from_parent, to_parent, n.ino];
                if let Some((name, mut old)) = destination {
                    inodes.push(old.ino);
                    if how == RenameMode::Exclusive { return Err(FsError::Exists); }
                    if old.entry.kind == Kind::Folder {
                        if n.entry.kind != Kind::Folder { return Err(FsError::IsDir); }
                        directory(tx, &drive, root, old.ino, offline)?;
                        if !names(tx, old.ino)?.is_empty() { return Err(FsError::NotEmpty); }
                    } else if n.entry.kind == Kind::Folder { return Err(FsError::NotDir); }
                    entries.push(entry(&drive, &key(&to_key, &name, old.entry.kind == Kind::Folder)?, Op::Delete, &old)?);
                    overlay(tx, to_parent, &name, None, old.ino)?;
                    old.sync = publication::pending(tx, old.ino)?;
                    old.generation += 1;
                    save_node(tx, &old)?;
                }
                let mut rename = entry(&drive, &from, Op::Rename, &n)?;
                rename.to_key = Some(to.clone());
                rename.replace = false;
                entries.push(rename);
                overlay(tx, from_parent, &actual, None, n.ino)?;
                overlay(tx, to_parent, &to_name, Some(n.ino), n.ino)?;
                n.entry.name = if n.entry.kind == Kind::Folder { format!("{to_name}/") } else { to_name };
                n.sync = publication::pending(tx, n.ino)?;
                n.generation += 1;
                save_node(tx, &n)?;
                changed(tx, from_parent)?;
                if to_parent != from_parent { changed(tx, to_parent)?; }
                inodes.sort_unstable();
                inodes.dedup();
                let folder = n.entry.kind == Kind::Folder;
                let change = LocalChange { invalidations: vec![invalidation(from, folder), invalidation(to, folder)], inodes, namespace: true };
                Ok((((), change), entries))
            }).await;
            match result { Err(FsError::Again) => continue, other => return other }
        }
        Err(FsError::Again)
    }
}
