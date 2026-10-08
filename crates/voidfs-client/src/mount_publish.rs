// SPDX-License-Identifier: Apache-2.0
//! Publication acknowledgements reconcile durable local identity without discarding newer edits.

use super::*;
use std::path::PathBuf;
use std::collections::BTreeMap;
use crate::journal::{Entry, Op};
use voidfs_sdk::{Attributes, ObjectMeta};

#[derive(Clone)]
pub(crate) struct Object { pub drive: String, pub key: String, pub meta: ObjectMeta, pub attrs: Attributes, pub content: Option<PathBuf> }
pub(crate) struct LocalCopy { pub path: Option<PathBuf>, pub size: u64, pub attr: Attr, pub xattrs: BTreeMap<String, Vec<u8>> }
pub(crate) enum Completion {
    Success { version: Option<String>, object: Option<Object> },
    Conflict { status: u16, remote: Option<Object>, current_version: Option<String>, error: String, local: HashMap<Ino, LocalCopy> },
    Error { error: String },
    Cancelled,
}
#[derive(Clone)]
pub(crate) struct Report { pub ino: Ino, pub attr: Attr, pub published: Option<Attr>, pub clean: bool }

/// The two retained versions of a guarded save. Snapshot files stay private to the core.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Conflict {
    pub ino: Ino,
    pub entry_id: i64,
    pub base_version: Option<String>,
    pub remote: Option<Attr>,
    pub local_size: u64,
    pub local: Option<Attr>,
    pub local_xattrs: Option<BTreeMap<String, Vec<u8>>>,
    pub remote_attrs: Option<Attributes>,
    pub remote_missing: bool,
    pub local_retained: bool,
    pub remote_retained: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum ConflictSide { Local, Remote }

pub(crate) fn stamp(tx: &rusqlite::Transaction<'_>, entry: &Entry) -> rusqlite::Result<()> {
    if let Some(ino) = entry.mount_ino {
        if matches!(entry.op, Op::Put | Op::Folder | Op::Rename | Op::Delete) {
            tx.execute("UPDATE mount_overlay_publications SET entry_id=?2 WHERE ino=?1 AND entry_id IS NULL", params![ino, entry.id])?;
        }
        let root = tx.query_row("SELECT ino FROM mount_roots WHERE drive=?1",[&entry.drive],|r| r.get::<_,Ino>(0)).optional()?;
        if let Some(root) = root { inherit(tx,&entry.drive,root,ino)?; }
    }
    Ok(())
}

pub(crate) fn inherit(c: &Connection, drive: &str, root: Ino, ino: Ino) -> rusqlite::Result<()> {
    if let Some(path) = chain(c,drive,root,ino)? {
        for (ancestor,_) in path.into_iter().rev().skip(1) {
            let owner = c.query_row("SELECT ino FROM mount_conflicts WHERE ino=?1 UNION ALL SELECT owner FROM mount_conflict_blockers WHERE ino=?1",[ancestor],|r| r.get::<_,Ino>(0)).optional()?;
            if let Some(owner) = owner {
                c.execute("INSERT INTO mount_conflict_blockers(ino,owner) VALUES (?1,?2) ON CONFLICT(ino) DO NOTHING",params![ino,owner])?;
                c.execute("UPDATE mount_inodes SET sync='conflict' WHERE ino=?1",[ino])?;
                break;
            }
        }
    }
    Ok(())
}

/// Legacy mount saves left overlays after their journal entries finished. Only assign an
/// owner when persisted namespace or journal evidence identifies the local inode.
pub(crate) fn recover_overlays(c: &Connection) -> rusqlite::Result<()> {
    let mut q = c.prepare("SELECT o.parent,o.name,o.ino,n.drive,r.ino FROM mount_overlay o JOIN mount_inodes n ON n.ino=o.parent
        JOIN mount_roots r ON r.drive=n.drive WHERE NOT EXISTS(SELECT 1 FROM mount_overlay_publications p WHERE p.parent=o.parent AND p.name=o.name)")?;
    let rows = q.query_map([], |r| Ok((r.get::<_, Ino>(0)?,r.get::<_, String>(1)?,r.get::<_, Option<Ino>>(2)?,r.get::<_, String>(3)?,r.get::<_, Ino>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() { return Ok(()); }
    let entries = crate::journal::all(c)?;
    for (parent, name, live, drive, root) in rows {
        let mut owner = live.or(c.query_row("SELECT ino FROM mount_names WHERE parent=?1 AND name=?2", params![parent,name], |r| r.get(0)).optional()?);
        if owner.is_none() {
            let mut keys = Vec::new();
            if let Ok(prefix) = remote_path(c,&drive,root,parent) { keys.push(format!("{prefix}{name}")); }
            if let Some(path) = chain(c,&drive,root,parent)? { keys.push(path.into_iter().skip(1).map(|(_,name)| name).chain(std::iter::once(name.clone())).collect::<Vec<_>>().join("/")); }
            owner = entries.iter().rev().find(|entry| entry.mount && entry.drive == drive && matches!(entry.op,Op::Delete | Op::Rename)
                && keys.iter().any(|key| entry.key.trim_end_matches('/') == key)).and_then(|entry| entry.mount_ino);
        }
        if let Some(owner) = owner {
            let id = c.query_row("SELECT entry_id FROM mount_inodes WHERE ino=?1 AND drive=?2",params![owner,drive],|r| r.get::<_,Option<i64>>(0)).optional()?.flatten();
            c.execute("INSERT INTO mount_overlay_publications(parent,name,ino,entry_id) VALUES (?1,?2,?3,?4)",params![parent,name,owner,id])?;
        }
    }
    Ok(())
}

pub(crate) fn pending(c: &Connection, ino: Ino) -> Result<Sync> {
    if c.query_row("SELECT 1 FROM mount_conflicts WHERE ino=?1 UNION ALL SELECT 1 FROM mount_conflict_blockers WHERE ino=?1", [ino], |_| Ok(())).optional()?.is_some() { return Ok(Sync::Conflict); }
    let entry: Option<i64> = c.query_row("SELECT entry_id FROM mount_inodes WHERE ino=?1", [ino], |r| r.get(0))?;
    let blocked = entry.map(|id| blocked(c, id)).transpose()?.flatten();
    Ok(match blocked { Some((_, true)) => Sync::Conflict, Some(_) => Sync::Error, None => Sync::Pending })
}

fn blocked(c: &Connection, id: i64) -> Result<Option<(i64, bool)>> {
    Ok(c.query_row("WITH RECURSIVE lineage(id, base, state, conflict) AS (
        SELECT id, base, state, conflict FROM entries WHERE id=?1
        UNION SELECT e.id, e.base, e.state, e.conflict FROM entries e JOIN lineage l ON l.base='e:'||e.id)
        SELECT id, conflict IS NOT NULL FROM lineage WHERE state IN ('failed', 'cancelled') ORDER BY conflict IS NULL LIMIT 1",
        [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
}

fn snapshot(object: &Object, ino: Ino, generation: u64) -> Result<Attr> {
    if object.meta.object_id.as_deref() != Some(object.attrs.object_id.as_str()) || object.attrs.object_id.is_empty()
        || object.attrs.version_id.as_deref() != Some(object.meta.version_id.as_str()) || object.attrs.kind != object.meta.kind
    { return Err(FsError::Io("publication returned inconsistent object identity or version".into())); }
    let entry = FolderEntry { name: object.key.rsplit('/').find(|part| !part.is_empty()).unwrap_or("").into(),
        kind: object.attrs.kind, object_id: object.attrs.object_id.clone(), version_id: Some(object.meta.version_id.clone()),
        size: (object.attrs.kind == Kind::File).then_some(object.meta.size), etag: Some(object.meta.etag.clone()),
        mtime: object.attrs.mtime.clone().or_else(|| object.meta.mtime.clone()), mode: object.attrs.mode.clone().or_else(|| object.meta.mode.clone()),
        has_xattrs: !object.attrs.xattrs.is_empty(), target: None };
    Node { ino, entry, generation, sync: Sync::Saved, entry_id: None, remote_key: None }.attr()
}

pub(crate) fn validate(object: &Object) -> Result<()> {
    use base64::Engine;
    snapshot(object,0,0)?;
    let mut bytes = 0usize;
    for (name,value) in &object.attrs.xattrs {
        if name.is_empty() || name.len() > 255 || name.contains('\0') { return Err(FsError::Io("invalid publication extended attribute name".into())); }
        let value = base64::engine::general_purpose::STANDARD.decode(value).map_err(|_| FsError::Io("invalid publication extended attribute base64".into()))?;
        bytes = bytes.checked_add(name.len()).and_then(|bytes| bytes.checked_add(value.len())).ok_or(FsError::TooLarge)?;
        if bytes > voidfs_core::model::MAX_XATTR_BYTES { return Err(FsError::TooLarge); }
    }
    Ok(())
}

fn related(c: &Connection, run: &[Entry]) -> rusqlite::Result<Vec<Ino>> {
    let mut inodes = BTreeSet::new();
    let mut entries = run.iter().filter(|entry| entry.mount).cloned().collect::<Vec<_>>();
    let mut ids = entries.iter().map(|entry| entry.id).collect::<HashSet<_>>();
    let pending = crate::journal::unfinished(c)?;
    loop {
        let more = pending.iter().filter(|entry| entry.mount && !ids.contains(&entry.id) && entries.iter().any(|earlier| {
            matches!(entry.base, crate::journal::StoredBase::Entry(id) if id == earlier.id)
                || earlier.id < entry.id && crate::queue::depends(earlier, entry)
        })).cloned().collect::<Vec<_>>();
        if more.is_empty() { break; }
        for entry in more { ids.insert(entry.id); entries.push(entry); }
    }
    inodes.extend(entries.into_iter().filter_map(|entry| entry.mount_ino));
    Ok(inodes.into_iter().collect())
}

fn bind_identity(c: &Connection, drive: &str, ino: Ino, object: &Object) -> Result<()> {
    let duplicate = c.query_row("SELECT ino FROM mount_inodes WHERE drive=?1 AND object_id=?2 AND ino<>?3",
        params![drive, object.attrs.object_id, ino], |r| r.get::<_, Ino>(0)).optional()?;
    if let Some(other) = duplicate {
        let n = node(c, drive, other)?.ok_or(FsError::Stale)?;
        if n.sync != Sync::Saved { return Err(FsError::Io("published identity belongs to another pending local inode".into())); }
        c.execute("UPDATE mount_inodes SET object_id=NULL WHERE ino=?1", [other])?;
        if object.attrs.kind == Kind::Folder {
            c.execute("DELETE FROM mount_names WHERE parent=?1 AND name IN (SELECT name FROM mount_names WHERE parent=?2)", params![other, ino])?;
            c.execute("UPDATE mount_names SET parent=?2 WHERE parent=?1", params![other, ino])?;
            c.execute("UPDATE mount_dirs SET listed=(SELECT listed FROM mount_dirs WHERE ino=?2), seq=(SELECT seq FROM mount_dirs WHERE ino=?2),
                generation=generation+1 WHERE ino=?1 AND EXISTS (SELECT 1 FROM mount_dirs WHERE ino=?2)", params![ino, other])?;
        }
        c.execute("DELETE FROM mount_names WHERE ino=?1", [other])?;
    }
    c.execute("UPDATE mount_inodes SET object_id=?2 WHERE ino=?1", params![ino, object.attrs.object_id])?;
    Ok(())
}

fn adopt_names(c: &Connection, entry: &Entry) -> Result<()> {
    let mut q = c.prepare("SELECT p.parent, p.name, o.ino FROM mount_overlay_publications p JOIN mount_overlay o
        ON o.parent=p.parent AND o.name=p.name WHERE p.entry_id=?1")?;
    let rows = q.query_map([entry.id], |r| Ok((r.get::<_, Ino>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<Ino>>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (parent, name, ino) in rows {
        c.execute("DELETE FROM mount_names WHERE parent=?1 AND name=?2", params![parent, name])?;
        if let Some(ino) = ino {
            c.execute("DELETE FROM mount_names WHERE ino=?1", [ino])?;
            c.execute("INSERT INTO mount_names(parent,name,ino,nfc) VALUES (?1,?2,?3,?4)", params![parent, name, ino, name.nfc().collect::<String>()])?;
        }
        c.execute("DELETE FROM mount_overlay WHERE parent=?1 AND name=?2", params![parent, name])?;
        c.execute("DELETE FROM mount_overlay_publications WHERE parent=?1 AND name=?2 AND entry_id=?3", params![parent, name, entry.id])?;
        c.execute("UPDATE mount_dirs SET listed=0,generation=generation+1 WHERE ino=?1", [parent])?;
    }
    Ok(())
}

fn moved_descendants(c: &Connection, entry: &Entry) -> Result<()> {
    let Some(to) = &entry.to_key else { return Ok(()); };
    if !entry.key.ends_with('/') { return Ok(()); }
    let end = format!("{}0", entry.key.trim_end_matches('/'));
    c.execute("UPDATE mount_inodes SET remote_key=?3||substr(remote_key,length(?2)+1),generation=generation+1
        WHERE drive=?1 AND remote_key>=?2 AND remote_key<?4", params![entry.drive, entry.key, to, end])?;
    Ok(())
}

pub(crate) fn reconcile(tx: &rusqlite::Transaction<'_>, run: &[Entry], completion: &Completion) -> Result<Vec<Report>> {
    let Some(first) = run.iter().find(|entry| entry.mount) else { return Ok(Vec::new()); };
    let inodes = if matches!(completion,Completion::Success { .. }) {
        run.iter().filter(|entry| entry.mount).filter_map(|entry| entry.mount_ino).collect::<BTreeSet<_>>().into_iter().collect()
    } else { related(tx,run)? };
    let mut reports = Vec::new();
    for ino in inodes {
        let Some(mut n) = node(tx, &first.drive, ino)? else { continue };
        let owned = run.iter().filter(|entry| entry.mount_ino == Some(ino)).collect::<Vec<_>>();
        let latest = n.entry_id.is_some_and(|id| owned.iter().any(|entry| entry.id == id));
        let mut published = None;
        let mut clean = false;
        match completion {
            Completion::Success { version, object } if !owned.is_empty() => {
                if owned.iter().any(|entry| entry.op == Op::Rename) {
                    for entry in &owned { if entry.op == Op::Rename { moved_descendants(tx, entry)?; } }
                }
                if let Some(object) = object {
                    let attr = snapshot(object, ino, n.generation.saturating_add(1))?;
                    if object.attrs.kind != n.entry.kind { return Err(FsError::Io("publication changed an existing local object kind".into())); }
                    if !n.entry.object_id.is_empty() && Some(n.entry.object_id.as_str()) != attr.object_id.as_deref() {
                        return Err(FsError::Io("publication changed an existing local object's identity".into()));
                    }
                    bind_identity(tx, &first.drive, ino, object)?;
                    n.entry.object_id = object.attrs.object_id.clone();
                    n.entry.version_id = version.clone();
                    n.entry.etag = Some(object.meta.etag.clone());
                    n.remote_key = Some(object.key.clone());
                    tx.execute("UPDATE mount_inodes SET remote_key=?2 WHERE ino=?1", params![ino, object.key])?;
                    published = Some(attr);
                } else if owned.iter().any(|entry| entry.op != Op::Delete) {
                    return Err(FsError::Io("published mount edit has no exact object snapshot".into()));
                }
                for entry in &owned { adopt_names(tx, entry)?; }
                let json: Option<String> = tx.query_row("SELECT record FROM mount_staged WHERE ino=?1", [ino], |r| r.get(0)).optional()?;
                let dirty = json.map(|json| serde_json::from_str::<stage::Record>(&json).map(|record| record.revision != record.flushed)).transpose()
                    .map_err(|e| FsError::Io(e.to_string()))?.unwrap_or(false);
                let unfinished: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM entries WHERE mount_ino=?1 AND state NOT IN ('done','cancelled'))", [ino], |r| r.get(0))?;
                let blocked = n.entry_id.map(|id| blocked(tx, id)).transpose()?.flatten();
                n.sync = match blocked { Some((_, true)) => Sync::Conflict, Some(_) => Sync::Error,
                    None if latest && !unfinished && !dirty => Sync::Saved, None if dirty => Sync::Pending, None => Sync::Saving };
                if n.sync == Sync::Saved {
                    if let Some(attr) = &published {
                        n.entry.size = (n.entry.kind == Kind::File).then_some(attr.size);
                        n.entry.mtime = object.as_ref().and_then(|o| o.attrs.mtime.clone().or_else(|| o.meta.mtime.clone()));
                        n.entry.mode = object.as_ref().and_then(|o| o.attrs.mode.clone().or_else(|| o.meta.mode.clone()));
                        n.entry.has_xattrs = attr.has_xattrs;
                    }
                    tx.execute("DELETE FROM mount_staged WHERE ino=?1", [ino])?;
                    tx.execute("UPDATE mount_inodes SET entry_id=NULL WHERE ino=?1", [ino])?;
                    if let Some(object) = object {
                        let attrs = object.attrs.xattrs.iter().map(|(name, value)| {
                            use base64::Engine;
                            base64::engine::general_purpose::STANDARD.decode(value).map(|bytes| (name.clone(), bytes))
                        }).collect::<std::result::Result<std::collections::BTreeMap<_, _>, _>>().map_err(|e| FsError::Io(e.to_string()))?;
                        tx.execute("UPDATE mount_xattrs SET attrs=?2,version=?3,dirty=0 WHERE ino=?1", params![ino, serde_json::to_string(&attrs).expect("JSON"), version])?;
                    }
                    clean = true;
                }
            }
            Completion::Conflict { .. } if owned.is_empty() => {
                n.sync = Sync::Conflict;
                if let Some(owner) = first.mount_ino {
                    tx.execute("INSERT INTO mount_conflict_blockers(ino,owner) VALUES (?1,?2) ON CONFLICT(ino) DO NOTHING", params![ino, owner])?;
                }
            }
            Completion::Conflict { status, remote, current_version, error, local } => {
                n.sync = Sync::Conflict;
                let remote_attr = remote.as_ref().map(|remote| snapshot(remote, ino, n.generation)).transpose()?;
                let copy = local.get(&ino);
                let entry = owned.first().copied().unwrap_or(first);
                let base_version = match &entry.base { crate::journal::StoredBase::Version(v) => Some(v.clone()), _ => n.entry.version_id.clone() };
                let snapshot_error = copy.is_none() || *status != 404 && remote.as_ref().is_none_or(|o| o.attrs.kind == Kind::File && o.content.is_none());
                tx.execute("INSERT INTO mount_conflicts(ino,entry_id,base_version,remote,remote_path,local_path,local_size,remote_missing,error,remote_attrs,local_attrs,local_xattrs)
                    VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(ino) DO NOTHING", params![ino, entry.id, base_version,
                    remote_attr.as_ref().map(|attr| serde_json::to_string(attr).expect("JSON")), remote.as_ref().and_then(|r| r.content.as_ref()).map(|p| p.display().to_string()),
                    copy.and_then(|c| c.path.as_ref()).map(|p| p.display().to_string()), copy.map(|c| c.size).unwrap_or(n.entry.size.unwrap_or(0)), *status == 404,
                    snapshot_error.then(|| format!("{error}; competing snapshot capture incomplete (version {})", current_version.as_deref().unwrap_or("unknown"))),
                    remote.as_ref().map(|o| serde_json::to_string(&o.attrs).expect("JSON")),
                    copy.map(|c| serde_json::to_string(&c.attr).expect("JSON")), copy.map(|c| serde_json::to_string(&c.xattrs).expect("JSON"))])?;
            }
            Completion::Error { error } => { let _ = error; n.sync = if pending(tx, ino)? == Sync::Conflict { Sync::Conflict } else { Sync::Error }; }
            Completion::Cancelled => { n.sync = if pending(tx, ino)? == Sync::Conflict { Sync::Conflict } else { Sync::Error }; }
            Completion::Success { .. } => {},
        }
        n.generation = n.generation.saturating_add(1);
        save_node(tx, &n)?;
        reports.push(Report { ino, attr: n.attr()?, published, clean });
    }
    Ok(reports)
}

async fn destination(store: &Store, size: u64) -> Result<(PathBuf, tokio::fs::File)> {
    use std::io::Read;
    let dir = store.dir().join("mount-conflicts");
    tokio::fs::create_dir_all(&dir).await?;
    let free = fs4::available_space(&dir)?;
    if free < size.checked_add(256 * 1024 * 1024).ok_or(FsError::NoSpace)? { return Err(FsError::NoSpace); }
    let mut random = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let path = dir.join(hex::encode(random));
    let file = tokio::fs::OpenOptions::new().create_new(true).write(true).open(&path).await?;
    Ok((path, file))
}

async fn synchronize(path: &std::path::Path, file: &tokio::fs::File) -> Result<()> {
    file.sync_all().await?;
    let dir = path.parent().ok_or_else(|| FsError::Io("conflict snapshot has no parent".into()))?.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::File::open(&dir)?.sync_all()?;
        std::fs::File::open(dir.parent().ok_or_else(|| FsError::Io("conflict snapshot has no state directory".into()))?)?.sync_all()?;
        Ok::<_, FsError>(())
    }).await.map_err(Error::from)?
}

pub(crate) async fn capture_remote(client: &Client, store: &Store, mut object: Object) -> Result<Object> {
    use tokio::io::AsyncWriteExt;
    if object.attrs.kind == Kind::Folder { return Ok(object); }
    let (path, mut file) = destination(store, object.meta.size).await?;
    let result = async {
        if object.meta.kind == Kind::File && object.meta.size > 0 {
            let mut body = client.get_object_stream(&object.drive, &object.key,
                voidfs_sdk::ReadOptions { version_id: Some(object.meta.version_id.clone()), ..Default::default() }).await.map_err(|e| sdk_error(&e))?;
            if body.meta.object_id != object.meta.object_id || body.meta.version_id != object.meta.version_id || body.meta.etag != object.meta.etag {
                return Err(FsError::Io("competing snapshot returned a different object or version".into()));
            }
            let mut copied = 0u64;
            while let Some(bytes) = body.chunk().await.map_err(|e| sdk_error(&e))? { file.write_all(&bytes).await?; copied += bytes.len() as u64; }
            if copied != object.meta.size { return Err(FsError::Io("competing remote snapshot has an unexpected size".into())); }
        }
        synchronize(&path, &file).await
    }.await;
    if let Err(error) = result { let _ = tokio::fs::remove_file(path).await; return Err(error); }
    object.content = Some(path);
    Ok(object)
}

pub(crate) async fn capture_local(client: &Client, store: &Arc<Store>, run: &[Entry], completion: &mut Completion, connectivity: &Connectivity) -> Result<()> {
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
    let Completion::Conflict { local, .. } = completion else { return Ok(()); };
    let Some(first) = run.iter().find(|entry| entry.mount) else { return Ok(()); };
    let (drive, run) = (first.drive.clone(), run.to_vec());
    let db_run = run.clone();
    let s = store.clone();
    let rows = tokio::task::spawn_blocking(move || s.with(|c| {
        let rows = db_run.iter().filter(|entry| entry.mount).filter_map(|entry| entry.mount_ino).collect::<BTreeSet<_>>().into_iter().map(|ino| {
            let n = node(c, &drive, ino)?.ok_or(rusqlite::Error::InvalidQuery)?;
            if !db_run.iter().any(|e| e.mount_ino == Some(ino))
                || c.query_row("SELECT local_path FROM mount_conflicts WHERE ino=?1", [ino], |r| r.get::<_, Option<String>>(0)).optional()?.flatten().is_some() { return Ok(None); }
            let staged = c.query_row("SELECT path,record FROM mount_staged WHERE ino=?1", [ino], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).optional()?;
            let xattrs = c.query_row("SELECT attrs FROM mount_xattrs WHERE ino=?1", [ino], |r| r.get::<_, String>(0)).optional()?;
            Ok(Some((ino, n, staged, xattrs)))
        }).collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((drive, rows))
    })).await.map_err(Error::from)??;
    let (drive, rows) = rows;
    let rows = rows.into_iter().flatten();
    let mut last_error = None;
    for (ino, n, staged, xattrs) in rows {
        let record = staged.as_ref().map(|(_, json)| serde_json::from_str::<stage::Record>(json)).transpose().map_err(|e| FsError::Io(e.to_string()))?;
        let size = record.as_ref().map(|r| r.size).unwrap_or(n.entry.size.unwrap_or(0));
        let result = async {
            let attr = n.attr()?;
            let base = record.as_ref().map(|r| r.base.clone()).unwrap_or(attr.clone());
            let mut key = n.remote_key.clone().or_else(|| record.as_ref().map(|r| r.key.clone())).unwrap_or_else(|| run_key_for_inode(&run, ino));
            let xattrs = match &xattrs {
                Some(json) => serde_json::from_str(json).map_err(|e| FsError::Io(e.to_string()))?,
                None if attr.has_xattrs => {
                    use base64::Engine;
                    connectivity.check()?;
                    let opts = voidfs_sdk::ReadOptions { version_id: attr.version_id.clone(), ..Default::default() };
                    let attrs = match client.attributes(&drive,&key,opts.clone()).await {
                        Err(error) if error.status() == Some(404) => {
                            key = locate_local(client,store,&drive,ino,&base,&key,connectivity).await?;
                            client.attributes(&drive,&key,opts).await
                        }
                        result => result,
                    }.map_err(|e| sdk_error(&e))?;
                    if Some(attrs.object_id.as_str()) != attr.object_id.as_deref() || attrs.version_id != attr.version_id { return Err(FsError::Io("local conflict attributes returned a different object or version".into())); }
                    attrs.xattrs.into_iter().map(|(name, value)| base64::engine::general_purpose::STANDARD.decode(value).map(|value| (name, value)))
                        .collect::<std::result::Result<BTreeMap<_, _>, _>>().map_err(|e| FsError::Io(e.to_string()))?
                }
                None => BTreeMap::new(),
            };
            if attr.kind == Kind::Folder { return Ok(LocalCopy { path: None, size, attr, xattrs }); }
            let (path, mut file) = destination(store, size).await?;
            let copied = async {
                file.set_len(size).await?;
                let remote_size = record.as_ref().map(|r| r.remote_size).unwrap_or(base.size);
                if base.kind == Kind::File && base.object_id.is_some() && remote_size > 0 {
                    let version = base.version_id.clone().ok_or_else(|| FsError::Io("local competing view has no pinned remote base".into()))?;
                    connectivity.check()?;
                    let opts = voidfs_sdk::ReadOptions { version_id: Some(version), ..Default::default() };
                    let mut body = match client.get_object_stream(&drive,&key,opts.clone()).await {
                        Err(error) if error.status() == Some(404) => {
                            key = locate_local(client,store,&drive,ino,&base,&key,connectivity).await?;
                            client.get_object_stream(&drive,&key,opts).await
                        }
                        result => result,
                    }.map_err(|e| sdk_error(&e))?;
                    if body.meta.object_id != base.object_id || body.meta.version_id != base.version_id.as_deref().unwrap_or("") {
                        return Err(FsError::Io("local conflict base returned a different object or version".into()));
                    }
                    let mut copied = 0u64;
                    while copied < remote_size {
                        let bytes = body.chunk().await.map_err(|e| sdk_error(&e))?.ok_or_else(|| FsError::Io("local conflict base is incomplete".into()))?;
                        let take = bytes.len().min((remote_size - copied) as usize);
                        file.write_all(&bytes[..take]).await?;
                        copied += take as u64;
                    }
                }
                if let (Some(record), Some((source, _))) = (&record, &staged) {
                    for extent in &record.extents {
                        let mut at = extent.start;
                        while at < extent.end {
                            let size = (extent.end - at).min(1024 * 1024);
                            let (source, physical) = (PathBuf::from(source), extent.physical + at - extent.start);
                            let bytes = tokio::task::spawn_blocking(move || stage::read_range(&source, physical, size)).await.map_err(Error::from)??;
                            file.seek(std::io::SeekFrom::Start(at)).await?;
                            file.write_all(&bytes).await?;
                            at += size;
                        }
                    }
                }
                synchronize(&path, &file).await
            }.await;
            if let Err(error) = copied { let _ = tokio::fs::remove_file(path).await; return Err(error); }
            Ok::<_, FsError>(LocalCopy { path: Some(path), size, attr, xattrs })
        }.await;
        match result { Ok(copy) => { local.insert(ino, copy); }, Err(error) => last_error = Some(error) }
    }
    match last_error { Some(error) => Err(error), None => Ok(()) }
}

async fn locate_local(client: &Client, store: &Arc<Store>, drive: &str, ino: Ino, base: &Attr, failed: &str, conn: &Connectivity) -> Result<String> {
    let (s,d) = (store.clone(),drive.to_owned());
    let known = tokio::task::spawn_blocking(move || s.with(|c| {
        let root = c.query_row("SELECT ino FROM mount_roots WHERE drive=?1",[&d],|r| r.get::<_,Ino>(0)).optional()?;
        Ok(root.and_then(|root| remote_path(c,&d,root,ino).ok()))
    })).await.map_err(Error::from)??;
    let resolve = async {
        if let Some(key) = known.filter(|key| key != failed) {
            conn.check()?;
            match client.head_object(drive,&key,voidfs_sdk::ReadOptions { version_id: base.version_id.clone(), ..Default::default() }).await {
                Ok(meta) if meta.object_id == base.object_id && Some(meta.version_id.as_str()) == base.version_id.as_deref() => return Ok(key),
                Ok(_) => return Err(FsError::Stale),
                Err(error) if error.status() == Some(404) => {},
                Err(error) => return Err(sdk_error(&error)),
            }
        }
        let id = base.object_id.as_deref().ok_or(FsError::Stale)?;
        let key = crate::mount_resolve::locate(client,drive,id,conn,&mut crate::mount_resolve::Budget::default()).await?.ok_or(FsError::Stale)?;
        if key == failed { return Err(FsError::Stale); }
        Ok(key)
    };
    tokio::time::timeout(Duration::from_secs(2),resolve).await.map_err(|_| FsError::Again)?
}

fn run_key_for_inode(run: &[Entry], ino: Ino) -> String { run.iter().find(|e| e.mount_ino == Some(ino)).map(|e| e.key.clone()).unwrap_or_default() }

fn decode_json<T: serde::de::DeserializeOwned>(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<Option<T>> {
    row.get::<_, Option<String>>(column)?.map(|json| serde_json::from_str(&json)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(e)))).transpose()
}

impl Session {
    pub async fn conflict(&self, ino: Ino) -> Result<Option<Conflict>> {
        self.db(move |c, drive, _| {
            c.query_row("SELECT x.entry_id,x.base_version,x.remote,x.local_size,x.remote_missing,x.local_path,x.remote_path,x.error,x.local_attrs,x.local_xattrs,x.remote_attrs,x.ino
                FROM mount_inodes n LEFT JOIN mount_conflict_blockers b ON b.ino=n.ino
                JOIN mount_conflicts x ON x.ino=coalesce(b.owner,n.ino) WHERE n.ino=?1 AND n.drive=?2", params![ino, drive], |r| {
                let remote: Option<String> = r.get(2)?;
                let remote = remote.map(|json| serde_json::from_str(&json).map_err(|e| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e)))).transpose()?;
                let missing: bool = r.get(4)?;
                Ok(Conflict { ino: r.get(11)?, entry_id: r.get(0)?, base_version: r.get(1)?, remote, local_size: r.get(3)?, local: decode_json(r, 8)?, local_xattrs: decode_json(r, 9)?, remote_attrs: decode_json(r, 10)?, remote_missing: missing,
                    local_retained: r.get::<_, Option<String>>(5)?.is_some() || decode_json::<Attr>(r,8)?.is_some_and(|attr| attr.kind == Kind::Folder),
                    remote_retained: missing || r.get::<_, Option<String>>(6)?.is_some() || decode_json::<Attr>(r,2)?.is_some_and(|attr| attr.kind == Kind::Folder), error: r.get(7)? })
            }).optional()
        }).await
    }

    pub async fn read_conflict(&self, ino: Ino, side: ConflictSide, offset: u64, len: u64) -> Result<Bytes> {
        if len > 8 * 1024 * 1024 || offset.checked_add(len).is_none() { return Err(FsError::InvalidArgument); }
        let conflict = self.conflict(ino).await?.ok_or(FsError::NotFound)?;
        let directory = match side { ConflictSide::Local => conflict.local.as_ref(), ConflictSide::Remote => conflict.remote.as_ref() }
            .is_some_and(|attr| attr.kind == Kind::Folder);
        if directory {
            return Err(FsError::IsDir);
        }
        let ino = conflict.ino;
        let path = self.db(move |c, drive, _| {
            c.query_row("SELECT x.local_path,x.remote_path,x.remote_missing FROM mount_conflicts x JOIN mount_inodes n ON n.ino=x.ino
                WHERE x.ino=?1 AND n.drive=?2", params![ino, drive], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, bool>(2)?))).optional()
        }).await?.ok_or(FsError::NotFound)?;
        let path = match side { ConflictSide::Local => path.0, ConflictSide::Remote if path.2 => return Err(FsError::NotFound), ConflictSide::Remote => path.1 }
            .ok_or_else(|| FsError::Io("competing snapshot is not retained locally".into()))?;
        let root = self.store.dir().join("mount-conflicts");
        let path = PathBuf::from(path);
        if !path.starts_with(root) { return Err(FsError::Io("invalid conflict snapshot path".into())); }
        tokio::task::spawn_blocking(move || {
            let size = std::fs::metadata(&path)?.len();
            if offset >= size { return Ok(Bytes::new()); }
            stage::read_range(&path, offset, len.min(size - offset)).map(Bytes::from)
        }).await.map_err(Error::from)?
    }
}
