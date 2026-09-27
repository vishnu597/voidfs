// SPDX-License-Identifier: Apache-2.0
//! Planning: turns protocol requests into transactions against a drive state (protocol §3–§4).
//!
//! A planner reads the state, checks preconditions and path rules, and returns the changes
//! that make one version. It never mutates the state; the authority applies the result.

use std::collections::{BTreeMap, HashMap};

use crate::ids::{ObjectId, Timestamp, VersionId};
use crate::model::{Actor, Attrs, Change, ContentDescriptor, CreateChange, Kind, MAX_XATTR_BYTES};
use crate::model::{MoveChange, ObjectRecord, Op, RemoveChange, SetChange, Txn};
use crate::names::Key;
use crate::state::DriveState;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpError {
    #[error("no such key")]
    NoSuchKey,
    #[error("no such version")]
    NoSuchVersion,
    #[error("a file and a folder cannot share a path, or the destination exists")]
    PathConflict,
    #[error("precondition failed")]
    PreconditionFailed { current: Option<VersionId> },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("too large: {0}")]
    TooLarge(String),
}

/// Request preconditions (protocol §4.0).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Precondition {
    pub if_version: Option<VersionId>,
    pub if_match: Option<String>,
    /// `If-None-Match: *`: the object must not exist.
    pub if_none_match_any: bool,
}

impl Precondition {
    pub fn is_empty(&self) -> bool {
        self.if_version.is_none() && self.if_match.is_none() && !self.if_none_match_any
    }

    pub fn check(&self, current: Option<&ObjectRecord>) -> Result<(), OpError> {
        let fail = || OpError::PreconditionFailed { current: current.map(|r| r.head) };
        match current {
            None if self.if_version.is_some() || self.if_match.is_some() => Err(fail()),
            Some(_) if self.if_none_match_any => Err(fail()),
            Some(r) if self.if_version.is_some_and(|v| v != r.head) => Err(fail()),
            Some(r) if self.if_match.as_ref().is_some_and(|m| !etag_matches(m, &r.etag)) => Err(fail()),
            _ => Ok(()),
        }
    }
}

/// `If-Match` accepts a list of ETags or `*`.
fn etag_matches(header: &str, etag: &str) -> bool {
    header.split(',').map(str::trim).any(|m| m == "*" || m == etag || m.trim_start_matches("W/") == etag)
}

/// Changes to attributes (protocol §4.8). Absent members are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttrsPatch {
    pub mtime: Option<Timestamp>,
    pub mode: Option<u32>,
    pub content_type: Option<String>,
    pub xattrs_set: BTreeMap<String, String>,
    pub xattrs_remove: Vec<String>,
    pub flags: Option<Vec<String>>,
}

impl AttrsPatch {
    pub fn is_empty(&self) -> bool {
        *self == AttrsPatch::default()
    }

    pub fn apply(&self, base: &Attrs) -> Result<Attrs, OpError> {
        let mut a = base.clone();
        if let Some(t) = self.mtime {
            a.mtime = Some(t);
        }
        if let Some(m) = self.mode {
            if m > 0o7777 {
                return Err(OpError::InvalidArgument(format!("mode {m:o} is out of range")));
            }
            a.mode = Some(m);
        }
        if let Some(ct) = &self.content_type {
            a.content_type = Some(ct.clone());
        }
        for name in &self.xattrs_remove {
            a.xattrs.remove(name);
        }
        for (name, value) in &self.xattrs_set {
            if name.is_empty() || name.len() > 255 {
                return Err(OpError::InvalidArgument("extended attribute names are 1 to 255 bytes".into()));
            }
            if !is_base64(value) {
                return Err(OpError::InvalidArgument(format!("extended attribute {name} is not base64")));
            }
            a.xattrs.insert(name.clone(), value.clone());
        }
        let total: usize = a.xattrs.iter().map(|(k, v)| k.len() + base64_decoded_len(v)).sum();
        if total > MAX_XATTR_BYTES {
            return Err(OpError::TooLarge("extended attributes exceed 64 KiB".into()));
        }
        if let Some(f) = &self.flags {
            a.flags = f.clone();
        }
        Ok(a)
    }
}

fn is_base64(s: &str) -> bool {
    s.len().is_multiple_of(4)
        && s.trim_end_matches('=').bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
        && s.len() - s.trim_end_matches('=').len() <= 2
}

fn base64_decoded_len(s: &str) -> usize {
    s.len() / 4 * 3 - (s.len() - s.trim_end_matches('=').len())
}

/// Builds one transaction, tracking folders it creates so later steps can see them.
struct Builder<'a> {
    state: &'a DriveState,
    changes: Vec<Change>,
    created: HashMap<(ObjectId, String), (ObjectId, Kind)>,
}

impl<'a> Builder<'a> {
    fn new(state: &'a DriveState) -> Self {
        Builder { state, changes: Vec::new(), created: HashMap::new() }
    }

    fn child(&self, parent: &ObjectId, name: &str) -> Option<(ObjectId, Kind)> {
        self.created.get(&(parent.clone(), name.to_owned())).cloned().or_else(|| self.state.child_named(parent, name))
    }

    fn create(&mut self, parent: &ObjectId, name: &str, kind: Kind, oid: ObjectId) -> ObjectId {
        self.changes.push(Change::Create(CreateChange { oid: oid.clone(), parent: parent.clone(), name: name.to_owned(), kind }));
        self.created.insert((parent.clone(), name.to_owned()), (oid.clone(), kind));
        oid
    }

    /// Resolves the folder path `names`, creating missing folders (protocol §1.2).
    fn folders(&mut self, names: &[String]) -> Result<ObjectId, OpError> {
        let mut cur = ObjectId::root();
        for name in names {
            cur = match self.child(&cur, name) {
                Some((oid, Kind::Folder)) => oid,
                Some(_) => return Err(OpError::PathConflict),
                None => self.create(&cur.clone(), name, Kind::Folder, ObjectId::generate()),
            };
        }
        Ok(cur)
    }

    fn finish(self, target: ObjectId, op: Op, actor: &Actor) -> Txn {
        Txn { target, op, actor: actor.clone(), changes: self.changes }
    }
}

fn parse(key: &str) -> Result<Key, OpError> {
    Key::parse(key).map_err(|e| OpError::InvalidArgument(e.to_string()))
}

fn current<'s>(state: &'s DriveState, key: &Key) -> Option<(ObjectId, &'s ObjectRecord)> {
    let oid = state.lookup(key)?;
    let r = state.record(&oid)?;
    Some((oid, r))
}

/// PutObject, CopyObject and completed multipart uploads: replaces a file's content and
/// attributes, creating it and its folders as needed. A folder key creates the folder.
pub fn put(
    state: &DriveState,
    key: &str,
    content: ContentDescriptor,
    attrs: Attrs,
    op: Op,
    pre: &Precondition,
    actor: &Actor,
) -> Result<Txn, OpError> {
    let key = parse(key)?;
    if key.is_root() {
        return Err(OpError::InvalidArgument("empty key".into()));
    }
    let mut b = Builder::new(state);
    if key.is_folder() {
        if content.size() != 0 {
            return Err(OpError::InvalidArgument("a folder key must have an empty body".into()));
        }
        let parent = b.folders(key.parent_names())?;
        let name = key.name().unwrap();
        let oid = match b.child(&parent, name) {
            Some((oid, Kind::Folder)) => {
                pre.check(state.record(&oid))?;
                let mut s = SetChange::new(oid.clone());
                s.attrs = Some(attrs);
                b.changes.push(Change::Set(s));
                oid
            }
            Some(_) => return Err(OpError::PathConflict),
            None => {
                pre.check(None)?;
                let oid = b.create(&parent, name, Kind::Folder, ObjectId::generate());
                let mut s = SetChange::new(oid.clone());
                s.attrs = Some(attrs);
                b.changes.push(Change::Set(s));
                oid
            }
        };
        return Ok(b.finish(oid, op, actor));
    }
    let parent = b.folders(key.parent_names())?;
    let name = key.name().unwrap();
    let oid = match b.child(&parent, name) {
        Some((oid, Kind::File)) => {
            pre.check(state.record(&oid))?;
            oid
        }
        Some(_) => return Err(OpError::PathConflict),
        None => {
            pre.check(None)?;
            b.create(&parent, name, Kind::File, ObjectId::generate())
        }
    };
    let mut s = SetChange::new(oid.clone());
    s.content = Some(content);
    s.attrs = Some(attrs);
    b.changes.push(Change::Set(s));
    Ok(b.finish(oid, op, actor))
}

/// The file an in-place edit applies to (protocol §4.1–§4.3). `None` means the edit creates it.
pub fn edit_target<'s>(state: &'s DriveState, key: &str, pre: &Precondition, create: bool) -> Result<Option<&'s ObjectRecord>, OpError> {
    let key = parse(key)?;
    if key.is_folder() {
        return Err(OpError::InvalidArgument("cannot edit a folder".into()));
    }
    match current(state, &key) {
        Some((_, r)) => {
            pre.check(Some(r))?;
            Ok(Some(r))
        }
        None if create => {
            pre.check(None)?;
            Ok(None)
        }
        None => Err(OpError::NoSuchKey),
    }
}

/// An in-place edit's result: the file's new content, as one `write` version. `attrs`
/// optionally changes mtime or mode at the same time.
pub fn write(
    state: &DriveState,
    key: &str,
    content: ContentDescriptor,
    attrs: &AttrsPatch,
    pre: &Precondition,
    actor: &Actor,
) -> Result<Txn, OpError> {
    let existing = edit_target(state, key, pre, true)?;
    match existing {
        Some(r) => {
            let mut s = SetChange::new(r.oid.clone());
            s.content = Some(content);
            if !attrs.is_empty() {
                s.attrs = Some(attrs.apply(&r.attrs)?);
            }
            let mut b = Builder::new(state);
            b.changes.push(Change::Set(s));
            Ok(b.finish(r.oid.clone(), Op::Write, actor))
        }
        None => put(state, key, content, attrs.apply(&Attrs::default())?, Op::Write, pre, actor),
    }
}

/// DeleteObject. `None` when there is nothing to do: the key does not exist, or it is a
/// folder that still has children (protocol §1.2).
pub fn delete(state: &DriveState, key: &str, pre: &Precondition, actor: &Actor) -> Result<Option<Txn>, OpError> {
    let key = parse(key)?;
    let Some((oid, r)) = current(state, &key) else { return Ok(None) };
    if key.is_root() || (r.kind == Kind::Folder && state.has_children(&oid)) {
        return Ok(None);
    }
    pre.check(Some(r))?;
    let mut b = Builder::new(state);
    b.changes.push(Change::Remove(RemoveChange { oid: oid.clone(), recursive: false }));
    Ok(Some(b.finish(oid, Op::Delete, actor)))
}

/// Rename or move (protocol §4.7).
pub fn rename(
    state: &DriveState,
    src: &str,
    dst: &str,
    replace: bool,
    attrs: &AttrsPatch,
    pre: &Precondition,
    actor: &Actor,
) -> Result<Txn, OpError> {
    let (src, dst) = (parse(src)?, parse(dst)?);
    if src.is_root() || dst.is_root() {
        return Err(OpError::InvalidArgument("cannot rename the root".into()));
    }
    if src.is_folder() != dst.is_folder() {
        return Err(OpError::InvalidArgument("source and destination must both be files or both folders".into()));
    }
    if src == dst {
        return Err(OpError::InvalidArgument("source and destination are the same".into()));
    }
    if src.is_folder() && src.contains(&dst) {
        return Err(OpError::InvalidArgument("a folder cannot move into itself".into()));
    }
    let (oid, r) = current(state, &src).ok_or(OpError::NoSuchKey)?;
    pre.check(Some(r))?;
    let mut b = Builder::new(state);
    let parent = b.folders(dst.parent_names())?;
    let name = dst.name().unwrap();
    match b.child(&parent, name) {
        None => {}
        Some((existing, Kind::File)) if replace && r.kind == Kind::File && existing != oid => {
            b.changes.push(Change::Remove(RemoveChange { oid: existing, recursive: false }));
        }
        Some(_) => return Err(OpError::PathConflict),
    }
    b.changes.push(Change::Move(MoveChange { oid: oid.clone(), parent, name: name.to_owned() }));
    if !attrs.is_empty() {
        let mut s = SetChange::new(oid.clone());
        s.attrs = Some(attrs.apply(&r.attrs)?);
        b.changes.push(Change::Set(s));
    }
    Ok(b.finish(oid, Op::Rename, actor))
}

/// Rollback to a version of the object at `key`, putting it back first if it was deleted
/// (protocol §4.6, §4.10).
pub fn restore(state: &DriveState, key: &str, version: VersionId, pre: &Precondition, actor: &Actor) -> Result<Txn, OpError> {
    let key = parse(key)?;
    let row = state.version(&version).ok_or(OpError::NoSuchVersion)?;
    let oid = row.oid.clone();
    let kind = state.kind(&oid).ok_or(OpError::NoSuchVersion)?;
    if key.is_root() || key.is_folder() != (kind == Kind::Folder) {
        return Err(OpError::NoSuchVersion);
    }
    let mut b = Builder::new(state);
    match state.lookup(&key) {
        Some(at) if at == oid => pre.check(state.record(&oid))?,
        Some(_) => return Err(OpError::NoSuchVersion),
        None if state.location(&oid).is_some() => return Err(OpError::NoSuchVersion),
        None => {
            pre.check(None)?;
            let parent = b.folders(key.parent_names())?;
            let name = key.name().unwrap();
            if b.child(&parent, name).is_some() {
                return Err(OpError::PathConflict);
            }
            b.create(&parent, name, kind, oid.clone());
        }
    }
    let mut s = SetChange::new(oid.clone());
    if kind == Kind::File {
        s.content = Some(row.content.clone().unwrap_or_else(ContentDescriptor::empty));
        s.etag = Some(row.etag.clone());
    }
    s.attrs = Some(row.attrs.clone());
    s.restored_from = Some(version);
    b.changes.push(Change::Set(s));
    Ok(b.finish(oid, Op::Restore, actor))
}

/// Attribute changes (protocol §4.8).
pub fn set_attrs(state: &DriveState, key: &str, patch: &AttrsPatch, pre: &Precondition, actor: &Actor) -> Result<Txn, OpError> {
    let key = parse(key)?;
    let (oid, r) = current(state, &key).ok_or(OpError::NoSuchKey)?;
    pre.check(Some(r))?;
    let mut s = SetChange::new(oid.clone());
    s.attrs = Some(patch.apply(&r.attrs)?);
    let mut b = Builder::new(state);
    b.changes.push(Change::Set(s));
    Ok(b.finish(oid, Op::Attrs, actor))
}

/// Restores a whole folder to its state at an earlier instant (protocol §4.6), as one version
/// of the folder. `then` is the drive's state at that instant.
///
/// Everything now inside the folder is taken out first (children before parents), then
/// everything that was inside it then is put back (parents before children) with its content
/// and attributes as they were. Doing it in that order means no name can collide half-way.
pub fn restore_subtree(now: &DriveState, then: &DriveState, key: &str, actor: &Actor) -> Result<Txn, OpError> {
    let key = parse(key)?;
    if !key.is_folder() || key.is_root() {
        return Err(OpError::InvalidArgument("subtree restore needs a folder key".into()));
    }
    let folder = now.lookup(&key).ok_or(OpError::NoSuchKey)?;
    if then.lookup(&key).as_ref() != Some(&folder) {
        return Err(OpError::NoSuchVersion);
    }
    let then_tree = then.subtree(&folder);
    let now_tree = now.subtree(&folder);
    let mut b = Builder::new(now);
    for (oid, ..) in now_tree.iter().rev() {
        b.changes.push(Change::Remove(RemoveChange { oid: oid.clone(), recursive: false }));
    }
    let taken_out: std::collections::HashSet<_> = now_tree.iter().map(|(o, ..)| o.clone()).collect();
    for (oid, parent, name, kind) in &then_tree {
        if now.location(oid).is_some() && !taken_out.contains(oid) {
            // It moved out of the folder since: bring it back.
            b.changes.push(Change::Move(MoveChange { oid: oid.clone(), parent: parent.clone(), name: name.clone() }));
        } else {
            b.changes.push(Change::Create(CreateChange { oid: oid.clone(), parent: parent.clone(), name: name.clone(), kind: *kind }));
        }
        let (Some(was), Some(is)) = (then.record(oid), now.record(oid)) else { continue };
        if was.etag != is.etag || was.attrs != is.attrs {
            let mut s = SetChange::new(oid.clone());
            if *kind == Kind::File {
                s.content = was.content.clone();
                s.etag = Some(was.etag.clone());
            }
            s.attrs = Some(was.attrs.clone());
            s.restored_from = Some(was.head);
            b.changes.push(Change::Set(s));
        }
    }
    if b.changes.is_empty() {
        b.changes.push(Change::Set(SetChange::new(folder.clone())));
    }
    let txn = b.finish(folder, Op::Restore, actor);
    now.check_txn(&txn).map_err(|e| OpError::InvalidArgument(format!("cannot restore this folder: {e}")))?;
    Ok(txn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ShardHash;
    use crate::model::{Commit, Extent};

    fn actor() -> Actor {
        Actor::system()
    }

    fn content(tag: &[u8], n: u64) -> ContentDescriptor {
        ContentDescriptor::Inline { extents: vec![Extent::Shard { s: ShardHash::of(tag), n }] }
    }

    /// Applies planned transactions one commit at a time.
    struct Drive {
        state: DriveState,
    }

    impl Drive {
        fn new() -> Self {
            Drive { state: DriveState::empty() }
        }

        fn run(&mut self, txn: Result<Txn, OpError>) -> Result<VersionId, OpError> {
            let txn = txn?;
            let seq = self.state.seq() + 1;
            let c = Commit { format: 1, seq, time: Timestamp::now(), authority: "t".into(), txns: vec![txn] };
            self.state = self.state.apply(&c).expect("planned transactions always apply");
            Ok(VersionId::new(seq, 0))
        }

        fn put(&mut self, key: &str, tag: &[u8]) -> Result<VersionId, OpError> {
            let t = put(&self.state, key, content(tag, tag.len() as u64), Attrs::default(), Op::Put, &Precondition::default(), &actor());
            self.run(t)
        }

        fn size(&self, key: &str) -> Option<u64> {
            self.state.lookup(&Key::parse(key).unwrap()).and_then(|o| self.state.record(&o)).map(|r| r.size)
        }
    }

    #[test]
    fn puts_create_parent_folders_and_detect_conflicts() {
        let mut d = Drive::new();
        d.put("a/b/c.txt", b"x").unwrap();
        assert!(d.state.lookup(&Key::parse("a/b/").unwrap()).is_some());
        assert_eq!(d.put("a", b"file where a folder is"), Err(OpError::PathConflict));
        assert_eq!(d.put("a/b/c.txt/d", b"under a file"), Err(OpError::PathConflict));
        let folder = put(&d.state, "empty/", ContentDescriptor::empty(), Attrs::default(), Op::Put, &Precondition::default(), &actor());
        d.run(folder).unwrap();
        assert_eq!(d.put("empty", b"clash"), Err(OpError::PathConflict));
    }

    #[test]
    fn preconditions_guard_writes() {
        let mut d = Drive::new();
        let v1 = d.put("p.txt", b"v1").unwrap();
        let only_new = Precondition { if_none_match_any: true, ..Default::default() };
        let t = put(&d.state, "p.txt", content(b"x", 1), Attrs::default(), Op::Put, &only_new, &actor());
        assert_eq!(t.unwrap_err(), OpError::PreconditionFailed { current: Some(v1) });
        let guarded = Precondition { if_version: Some(v1), ..Default::default() };
        let v2 = d.run(write(&d.state, "p.txt", content(b"V1", 2), &AttrsPatch::default(), &guarded, &actor())).unwrap();
        let stale = write(&d.state, "p.txt", content(b"VX", 2), &AttrsPatch::default(), &guarded, &actor());
        assert_eq!(stale.unwrap_err(), OpError::PreconditionFailed { current: Some(v2) });
        let missing = write(&d.state, "missing.txt", content(b"X", 1), &AttrsPatch::default(), &guarded, &actor());
        assert_eq!(missing.unwrap_err(), OpError::PreconditionFailed { current: None });
        let etag = d.state.record(&d.state.lookup(&Key::parse("p.txt").unwrap()).unwrap()).unwrap().etag.clone();
        let by_etag = Precondition { if_match: Some(etag), ..Default::default() };
        assert!(put(&d.state, "p.txt", content(b"y", 1), Attrs::default(), Op::Put, &by_etag, &actor()).is_ok());
    }

    #[test]
    fn deletes_leave_folders_and_skip_non_empty_ones() {
        let mut d = Drive::new();
        d.put("d/x.txt", b"x").unwrap();
        d.run(delete(&d.state, "d/x.txt", &Precondition::default(), &actor()).map(Option::unwrap)).unwrap();
        assert!(d.state.lookup(&Key::parse("d/").unwrap()).is_some());
        d.put("e/y.txt", b"y").unwrap();
        assert_eq!(delete(&d.state, "e/", &Precondition::default(), &actor()), Ok(None));
        assert_eq!(delete(&d.state, "nope", &Precondition::default(), &actor()), Ok(None));
    }

    #[test]
    fn renames_follow_the_rules() {
        let mut d = Drive::new();
        d.put("src/1.txt", b"1").unwrap();
        d.put("src/sub/2.txt", b"22").unwrap();
        let none = AttrsPatch::default();
        let pre = Precondition::default();
        d.run(rename(&d.state, "src/", "dst/", false, &none, &pre, &actor())).unwrap();
        assert_eq!(d.size("dst/sub/2.txt"), Some(2));
        assert_eq!(rename(&d.state, "dst/", "dst/inner/", false, &none, &pre, &actor()).unwrap_err(), OpError::InvalidArgument("a folder cannot move into itself".into()));
        assert!(matches!(rename(&d.state, "dst/", "file.txt", false, &none, &pre, &actor()), Err(OpError::InvalidArgument(_))));
        d.put("doc.txt", b"old").unwrap();
        d.put(".doc.tmp", b"newer").unwrap();
        assert_eq!(rename(&d.state, ".doc.tmp", "doc.txt", false, &none, &pre, &actor()).unwrap_err(), OpError::PathConflict);
        d.run(rename(&d.state, ".doc.tmp", "doc.txt", true, &none, &pre, &actor())).unwrap();
        assert_eq!(d.size("doc.txt"), Some(5));
        assert_eq!(d.size(".doc.tmp"), None);
        assert_eq!(rename(&d.state, "gone", "x", false, &none, &pre, &actor()).unwrap_err(), OpError::NoSuchKey);
    }

    #[test]
    fn restore_rolls_back_and_brings_deleted_files_back() {
        let mut d = Drive::new();
        let v1 = d.put("h.txt", b"one").unwrap();
        d.put("h.txt", b"two!").unwrap();
        let pre = Precondition::default();
        d.run(restore(&d.state, "h.txt", v1, &pre, &actor())).unwrap();
        assert_eq!(d.size("h.txt"), Some(3));
        let r = d.state.record(&d.state.lookup(&Key::parse("h.txt").unwrap()).unwrap()).unwrap();
        assert_eq!(r.restored_from, Some(v1));
        assert_eq!(restore(&d.state, "h.txt", VersionId::new(999, 0), &pre, &actor()).unwrap_err(), OpError::NoSuchVersion);

        let last = d.put("gone.txt", b"bye").unwrap();
        d.run(delete(&d.state, "gone.txt", &pre, &actor()).map(Option::unwrap)).unwrap();
        let row = d.state.removed("", None).next().unwrap().clone();
        assert_eq!(row.last_version, last);
        d.run(restore(&d.state, "gone.txt", row.last_version, &pre, &actor())).unwrap();
        assert_eq!(d.size("gone.txt"), Some(3));
        assert_eq!(d.state.removed("", None).count(), 0);
        // A version of one object cannot be restored over another.
        assert_eq!(restore(&d.state, "h.txt", last, &pre, &actor()).unwrap_err(), OpError::NoSuchVersion);
    }

    #[test]
    fn subtree_restore_returns_a_folder_to_an_earlier_state() {
        let mut d = Drive::new();
        d.put("p/a.txt", b"1").unwrap();
        d.put("p/b.txt", b"keep").unwrap();
        d.put("p/sub/c.txt", b"cc").unwrap();
        let then = d.state.clone();
        d.put("p/a.txt", b"22").unwrap();
        d.put("p/new.txt", b"n").unwrap();
        let pre = Precondition::default();
        d.run(delete(&d.state, "p/b.txt", &pre, &actor()).map(Option::unwrap)).unwrap();
        d.run(rename(&d.state, "p/sub/", "elsewhere/", false, &AttrsPatch::default(), &pre, &actor())).unwrap();
        // Swap two names inside the folder, the case that trips a naive in-place restore.
        d.put("p/x", b"x").unwrap();
        d.run(restore_subtree(&d.state, &then, "p/", &actor())).unwrap();
        assert_eq!(d.size("p/a.txt"), Some(1));
        assert_eq!(d.size("p/b.txt"), Some(4));
        assert_eq!(d.size("p/sub/c.txt"), Some(2));
        assert_eq!(d.size("p/new.txt"), None);
        assert_eq!(d.size("p/x"), None);
        assert_eq!(d.state.lookup(&Key::parse("elsewhere/").unwrap()), None);
        assert!(matches!(restore_subtree(&d.state, &then, "q/", &actor()), Err(OpError::NoSuchKey)));
    }

    #[test]
    fn attributes_are_validated() {
        let mut d = Drive::new();
        d.put("a.txt", b"a").unwrap();
        let pre = Precondition::default();
        let mut p = AttrsPatch { mode: Some(0o644), ..Default::default() };
        p.xattrs_set.insert("user.tag".into(), "aGk=".into());
        d.run(set_attrs(&d.state, "a.txt", &p, &pre, &actor())).unwrap();
        let r = d.state.record(&d.state.lookup(&Key::parse("a.txt").unwrap()).unwrap()).unwrap();
        assert_eq!(r.attrs.mode, Some(0o644));
        assert_eq!(r.attrs.xattrs["user.tag"], "aGk=");
        let mut big = AttrsPatch::default();
        big.xattrs_set.insert("user.big".into(), "A".repeat(90_000));
        assert!(matches!(set_attrs(&d.state, "a.txt", &big, &pre, &actor()), Err(OpError::TooLarge(_))));
        let mut bad = AttrsPatch::default();
        bad.xattrs_set.insert("user.bad".into(), "not base64!".into());
        assert!(matches!(set_attrs(&d.state, "a.txt", &bad, &pre, &actor()), Err(OpError::InvalidArgument(_))));
    }
}
