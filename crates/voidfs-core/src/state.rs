// SPDX-License-Identifier: Apache-2.0
//! A drive's state and how commits change it (format §7.5–§8).
//!
//! State is built from persistent maps, so cloning it is cheap: applying a commit works on a
//! clone and only replaces the original when every transaction succeeded, a fork starts as a
//! clone, and readers can hold a snapshot while writers move on.

use std::ops::Bound;

use imbl::{HashMap as IMap, OrdMap, Vector};

use crate::chunk::Shard;
use crate::ids::{ObjectId, Timestamp, VersionId};
use crate::model::{Commit, ContentDescriptor, EntryRow, HistoryRow, Kind, ObjectRecord, Op, RemovedRow, Txn};
use crate::model::{Change, CreateChange, MoveChange, RemoveChange, SetChange};
use crate::names::{self, Key};

/// Where a linked object sits: its parent folder and its name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Location {
    pub parent: ObjectId,
    pub name: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StateError {
    #[error("commit {got} does not follow {expected}")]
    BadSeq { expected: u64, got: u64 },
    #[error("commit time goes backwards")]
    TimeWentBackwards,
    #[error("transaction has no changes")]
    EmptyTxn,
    #[error("object {0} does not exist")]
    UnknownObject(ObjectId),
    #[error("object {0} is not a folder")]
    NotAFolder(ObjectId),
    #[error("{name:?} already exists in {parent}")]
    NameTaken { parent: ObjectId, name: String },
    #[error("object {0} is already in the namespace")]
    AlreadyLinked(ObjectId),
    #[error("object {0} is not in the namespace")]
    NotLinked(ObjectId),
    #[error("object {0} cannot be re-created with a different kind")]
    KindMismatch(ObjectId),
    #[error("folder {0} is not empty")]
    FolderNotEmpty(ObjectId),
    #[error("moving {0} there would put it inside itself")]
    Cycle(ObjectId),
    #[error("invalid name: {0}")]
    BadName(#[from] names::NameError),
    #[error("size {size} does not match content of {content} bytes")]
    SizeMismatch { size: u64, content: u64 },
    #[error("the root cannot be changed that way")]
    Root,
}

/// One item of a listing (protocol §3 ListObjectsV2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Listed {
    Object { key: String, oid: ObjectId },
    Prefix { key: String },
}

impl Listed {
    pub fn key(&self) -> &str {
        match self {
            Listed::Object { key, .. } | Listed::Prefix { key } => key,
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct DriveState {
    seq: u64,
    time: Option<Timestamp>,
    /// `(parent, segment)` → child. Segments order children so that a walk yields key order.
    entries: OrdMap<(ObjectId, Vec<u8>), ObjectId>,
    locations: IMap<ObjectId, Location>,
    objects: IMap<ObjectId, ObjectRecord>,
    history: IMap<ObjectId, Vector<HistoryRow>>,
    versions: IMap<VersionId, ObjectId>,
    removed: OrdMap<(String, ObjectId), RemovedRow>,
    removed_keys: IMap<ObjectId, String>,
}

impl DriveState {
    /// An empty drive whose log starts after `seq` (0 for a new drive, the fork point for a
    /// fork's own log).
    pub fn empty() -> Self {
        DriveState::default()
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn time(&self) -> Option<Timestamp> {
        self.time
    }

    /// Continues this state as a fork: same content and history, same position.
    pub fn fork(&self) -> Self {
        self.clone()
    }

    // -----------------------------------------------------------------------------------------
    // Applying commits

    /// Returns the state after `commit`, or an error and no change.
    pub fn apply(&self, commit: &Commit) -> Result<DriveState, StateError> {
        if commit.seq != self.seq + 1 {
            return Err(StateError::BadSeq { expected: self.seq + 1, got: commit.seq });
        }
        if self.time.is_some_and(|t| commit.time < t) {
            return Err(StateError::TimeWentBackwards);
        }
        let mut next = self.clone();
        for (i, txn) in commit.txns.iter().enumerate() {
            next.apply_txn(txn, VersionId::new(commit.seq, i as u32), commit.time)?;
        }
        next.seq = commit.seq;
        next.time = Some(commit.time);
        Ok(next)
    }

    /// Returns the state after `txn` as transaction `idx` of commit `seq` made at `time`, or an
    /// error and no change. Applying a commit's transactions this way one after another, from
    /// the state before it, gives what [`DriveState::apply`] gives; a writer that builds a commit
    /// of several transactions plans each against the state the ones before it leave (format
    /// §7.1). The result's position is the commit's from its first transaction on.
    pub fn apply_txn_of(&self, seq: u64, idx: u32, time: Timestamp, txn: &Txn) -> Result<DriveState, StateError> {
        let expected = if idx == 0 { self.seq + 1 } else { self.seq };
        if seq != expected {
            return Err(StateError::BadSeq { expected, got: seq });
        }
        if self.time.is_some_and(|t| time < t) || (idx > 0 && self.time != Some(time)) {
            return Err(StateError::TimeWentBackwards);
        }
        let mut next = self.clone();
        next.apply_txn(txn, VersionId::new(seq, idx), time)?;
        next.seq = seq;
        next.time = Some(time);
        Ok(next)
    }

    /// Checks that `txn` would apply cleanly to this state, without keeping the result.
    pub fn check_txn(&self, txn: &Txn) -> Result<(), StateError> {
        let mut scratch = self.clone();
        scratch.apply_txn(txn, VersionId::new(self.seq + 1, 0), self.time.unwrap_or_else(Timestamp::now))
    }

    fn apply_txn(&mut self, txn: &Txn, v: VersionId, time: Timestamp) -> Result<(), StateError> {
        if txn.changes.is_empty() {
            return Err(StateError::EmptyTxn);
        }
        let prior_head = self.objects.get(&txn.target).map(|r| r.head);
        let mut touched: Vec<ObjectId> = Vec::new();
        for change in &txn.changes {
            let oid = match change {
                Change::Create(c) => self.create(c, v, time)?,
                Change::Set(c) => self.set(c)?,
                Change::Move(c) => self.move_(c)?,
                Change::Remove(c) => self.remove(c, v, time, prior_head)?,
            };
            if !touched.contains(&oid) {
                touched.push(oid);
            }
        }
        for oid in &touched {
            if let Some(r) = self.objects.get_mut(oid) {
                r.head = v;
                r.time = time;
                if *oid == txn.target && txn.op != Op::Restore {
                    r.restored_from = None;
                }
            }
        }
        let r = self.objects.get(&txn.target).ok_or_else(|| StateError::UnknownObject(txn.target.clone()))?;
        let row = HistoryRow {
            oid: r.oid.clone(),
            version: v,
            time,
            op: txn.op,
            size: r.size,
            etag: r.etag.clone(),
            content: r.content.clone(),
            attrs: r.attrs.clone(),
            restored_from: r.restored_from,
            actor: txn.actor.clone(),
        };
        self.history.entry(txn.target.clone()).or_default().push_back(row);
        self.versions.insert(v, txn.target.clone());
        Ok(())
    }

    fn create(&mut self, c: &CreateChange, v: VersionId, time: Timestamp) -> Result<ObjectId, StateError> {
        if c.oid.is_root() {
            return Err(StateError::Root);
        }
        names::validate_name(&c.name)?;
        self.require_folder(&c.parent)?;
        self.require_free(&c.parent, &c.name, None)?;
        if let Some(existing) = self.objects.get(&c.oid) {
            // Putting a deleted object back (format §7.5).
            if self.locations.contains_key(&c.oid) {
                return Err(StateError::AlreadyLinked(c.oid.clone()));
            }
            if existing.kind != c.kind {
                return Err(StateError::KindMismatch(c.oid.clone()));
            }
            if self.is_ancestor(&c.oid, &c.parent) {
                return Err(StateError::Cycle(c.oid.clone()));
            }
            if let Some(key) = self.removed_keys.remove(&c.oid) {
                self.removed.remove(&(key, c.oid.clone()));
            }
        } else {
            let content = (c.kind == Kind::File).then(ContentDescriptor::empty);
            let etag = content.as_ref().map(ContentDescriptor::etag).unwrap_or_else(|| "\"\"".into());
            self.objects.insert(
                c.oid.clone(),
                ObjectRecord {
                    oid: c.oid.clone(),
                    kind: c.kind,
                    content,
                    size: 0,
                    etag,
                    attrs: Default::default(),
                    target: None,
                    restored_from: None,
                    head: v,
                    time,
                },
            );
        }
        self.link(&c.oid, c.kind, &c.parent, &c.name);
        Ok(c.oid.clone())
    }

    fn set(&mut self, c: &SetChange) -> Result<ObjectId, StateError> {
        let r = self.objects.get_mut(&c.oid).ok_or_else(|| StateError::UnknownObject(c.oid.clone()))?;
        if let Some(content) = &c.content {
            let n = content.size();
            if c.size.is_some_and(|s| s != n) {
                return Err(StateError::SizeMismatch { size: c.size.unwrap(), content: n });
            }
            r.content = Some(content.clone());
            r.size = n;
            r.etag = c.etag.clone().unwrap_or_else(|| content.etag());
        } else if let Some(s) = c.size {
            let n = r.content.as_ref().map_or(0, ContentDescriptor::size);
            if s != n {
                return Err(StateError::SizeMismatch { size: s, content: n });
            }
        }
        if c.content.is_none()
            && let Some(e) = &c.etag {
                r.etag = e.clone();
            }
        if let Some(a) = &c.attrs {
            r.attrs = a.clone();
        }
        if let Some(t) = &c.target {
            r.target = Some(t.clone());
        }
        if let Some(rf) = c.restored_from {
            r.restored_from = Some(rf);
        }
        Ok(c.oid.clone())
    }

    fn move_(&mut self, c: &MoveChange) -> Result<ObjectId, StateError> {
        if c.oid.is_root() {
            return Err(StateError::Root);
        }
        names::validate_name(&c.name)?;
        let kind = self.kind(&c.oid).ok_or_else(|| StateError::UnknownObject(c.oid.clone()))?;
        if !self.locations.contains_key(&c.oid) {
            return Err(StateError::NotLinked(c.oid.clone()));
        }
        self.require_folder(&c.parent)?;
        self.require_free(&c.parent, &c.name, Some(&c.oid))?;
        if self.is_ancestor(&c.oid, &c.parent) {
            return Err(StateError::Cycle(c.oid.clone()));
        }
        self.unlink(&c.oid, kind);
        self.link(&c.oid, kind, &c.parent, &c.name);
        Ok(c.oid.clone())
    }

    fn remove(&mut self, c: &RemoveChange, v: VersionId, time: Timestamp, prior_head: Option<VersionId>) -> Result<ObjectId, StateError> {
        if c.oid.is_root() {
            return Err(StateError::Root);
        }
        let r = self.objects.get(&c.oid).ok_or_else(|| StateError::UnknownObject(c.oid.clone()))?.clone();
        if !self.locations.contains_key(&c.oid) {
            return Err(StateError::NotLinked(c.oid.clone()));
        }
        if r.kind == Kind::Folder && self.has_children(&c.oid) && !c.recursive {
            return Err(StateError::FolderNotEmpty(c.oid.clone()));
        }
        let key = self.key_of(&c.oid).ok_or_else(|| StateError::NotLinked(c.oid.clone()))?;
        // A removed folder keeps its children attached to it, so putting it back restores the
        // whole subtree.
        self.unlink(&c.oid, r.kind);
        let row = RemovedRow {
            key: key.clone(),
            oid: c.oid.clone(),
            version: v,
            last_version: prior_head.unwrap_or(r.head),
            time,
            kind: r.kind,
            size: r.size,
        };
        self.removed.insert((key.clone(), c.oid.clone()), row);
        self.removed_keys.insert(c.oid.clone(), key);
        Ok(c.oid.clone())
    }

    fn link(&mut self, oid: &ObjectId, kind: Kind, parent: &ObjectId, name: &str) {
        let seg = names::segment(name, kind == Kind::Folder);
        self.entries.insert((parent.clone(), seg), oid.clone());
        self.locations.insert(oid.clone(), Location { parent: parent.clone(), name: name.to_owned() });
    }

    fn unlink(&mut self, oid: &ObjectId, kind: Kind) {
        if let Some(loc) = self.locations.remove(oid) {
            self.entries.remove(&(loc.parent, names::segment(&loc.name, kind == Kind::Folder)));
        }
    }

    fn require_folder(&self, oid: &ObjectId) -> Result<(), StateError> {
        match self.kind(oid) {
            Some(Kind::Folder) => Ok(()),
            Some(_) => Err(StateError::NotAFolder(oid.clone())),
            None => Err(StateError::UnknownObject(oid.clone())),
        }
    }

    /// A name may be used once per folder, whatever the kind (format §7.5 invariant 2).
    fn require_free(&self, parent: &ObjectId, name: &str, except: Option<&ObjectId>) -> Result<(), StateError> {
        match self.child_named(parent, name) {
            Some((oid, _)) if Some(&oid) != except => Err(StateError::NameTaken { parent: parent.clone(), name: name.to_owned() }),
            _ => Ok(()),
        }
    }

    /// Whether `ancestor` is `node` or one of its ancestors.
    fn is_ancestor(&self, ancestor: &ObjectId, node: &ObjectId) -> bool {
        let mut cur = node.clone();
        loop {
            if &cur == ancestor {
                return true;
            }
            match self.locations.get(&cur) {
                Some(loc) => cur = loc.parent.clone(),
                None => return false,
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Queries

    pub fn kind(&self, oid: &ObjectId) -> Option<Kind> {
        if oid.is_root() { Some(Kind::Folder) } else { self.objects.get(oid).map(|r| r.kind) }
    }

    pub fn record(&self, oid: &ObjectId) -> Option<&ObjectRecord> {
        self.objects.get(oid)
    }

    pub fn location(&self, oid: &ObjectId) -> Option<&Location> {
        self.locations.get(oid)
    }

    /// The child of `parent` called `name`, of either kind.
    pub fn child_named(&self, parent: &ObjectId, name: &str) -> Option<(ObjectId, Kind)> {
        let file = self.entries.get(&(parent.clone(), names::segment(name, false)));
        let folder = self.entries.get(&(parent.clone(), names::segment(name, true)));
        file.or(folder).map(|o| (o.clone(), self.kind(o).unwrap_or(Kind::File)))
    }

    /// Resolves a key to the object currently at it.
    pub fn lookup(&self, key: &Key) -> Option<ObjectId> {
        let mut cur = ObjectId::root();
        let n = key.names().len();
        for (i, name) in key.names().iter().enumerate() {
            let folder = i + 1 < n || key.is_folder();
            cur = self.entries.get(&(cur, names::segment(name, folder)))?.clone();
        }
        Some(cur)
    }

    /// The key an object is currently reachable at, if it is.
    pub fn key_of(&self, oid: &ObjectId) -> Option<String> {
        if oid.is_root() {
            return Some(String::new());
        }
        let mut parts = Vec::new();
        let mut cur = oid.clone();
        while !cur.is_root() {
            let loc = self.locations.get(&cur)?;
            parts.push(loc.name.clone());
            cur = loc.parent.clone();
        }
        parts.reverse();
        let mut key = parts.join("/");
        if self.kind(oid) == Some(Kind::Folder) {
            key.push('/');
        }
        Some(key)
    }

    /// A folder's children in key order: `(segment, child)`.
    pub fn children<'a>(&'a self, parent: &ObjectId) -> impl Iterator<Item = (&'a [u8], &'a ObjectId)> + 'a {
        let parent = parent.clone();
        self.entries
            .range((parent.clone(), Vec::new())..)
            .take_while(move |((p, _), _)| *p == parent)
            .map(|((_, seg), oid)| (seg.as_slice(), oid))
    }

    pub fn has_children(&self, oid: &ObjectId) -> bool {
        self.children(oid).next().is_some()
    }

    /// Every version of an object, oldest first.
    pub fn history(&self, oid: &ObjectId) -> Option<&Vector<HistoryRow>> {
        self.history.get(oid)
    }

    /// Finds a version anywhere in the drive.
    pub fn version(&self, v: &VersionId) -> Option<&HistoryRow> {
        let oid = self.versions.get(v)?;
        let rows = self.history.get(oid)?;
        let i = rows.binary_search_by(|r| r.version.cmp(v)).ok()?;
        rows.get(i)
    }

    /// The version of an object that was current at `t` (protocol §4.5).
    pub fn as_of(&self, oid: &ObjectId, t: Timestamp) -> Option<&HistoryRow> {
        let rows = self.history.get(oid)?;
        let n = rows.iter().take_while(|r| r.time <= t).count();
        if n == 0 { None } else { rows.get(n - 1) }
    }

    /// Objects that were removed and are not back, in key order, from `after` (exclusive).
    pub fn removed<'a>(&'a self, prefix: &'a str, after: Option<&'a str>) -> impl Iterator<Item = &'a RemovedRow> + 'a {
        let lower = match after {
            Some(a) => Bound::Excluded((a.to_owned(), ObjectId::root())),
            None => Bound::Included((prefix.to_owned(), ObjectId::root())),
        };
        self.removed
            .range((lower, Bound::Unbounded))
            .map(|(_, row)| row)
            .skip_while(move |r| r.key.as_str() < prefix)
            .take_while(move |r| r.key.starts_with(prefix))
    }

    /// The removed-row for an object that is out of the namespace.
    pub fn removed_row(&self, oid: &ObjectId) -> Option<&RemovedRow> {
        let key = self.removed_keys.get(oid)?;
        self.removed.get(&(key.clone(), oid.clone()))
    }

    /// ListObjectsV2 (protocol §1.2, §3): keys starting with `prefix`, after `after`, at most
    /// `max`. With `delimited`, direct children only, folders as prefixes. Without, files and
    /// empty folders of the whole subtree. Returns the items and whether more remain.
    pub fn list(&self, prefix: &str, delimited: bool, after: Option<&str>, max: usize) -> (Vec<Listed>, bool) {
        let split = prefix.rfind('/').map_or(0, |i| i + 1);
        let (base, rest) = prefix.split_at(split);
        let Ok(base_key) = Key::parse(base) else { return (Vec::new(), false) };
        let Some(base_oid) = self.lookup(&base_key) else { return (Vec::new(), false) };
        let past = |k: &str| after.is_none_or(|a| k > a);

        let mut out = Vec::new();
        if rest.is_empty() && !base.is_empty() && !self.has_children(&base_oid) && past(base) {
            out.push(Listed::Object { key: base.to_owned(), oid: base_oid.clone() });
        }

        struct Frame {
            oid: ObjectId,
            prefix: String,
            cursor: Option<Vec<u8>>,
            filter: Vec<u8>,
        }
        let mut stack =
            vec![Frame { oid: base_oid, prefix: base.to_owned(), cursor: None, filter: rest.as_bytes().to_vec() }];
        while out.len() <= max {
            let Some(frame) = stack.last_mut() else { break };
            let lower = match &frame.cursor {
                Some(c) => Bound::Excluded((frame.oid.clone(), c.clone())),
                None => Bound::Included((frame.oid.clone(), frame.filter.clone())),
            };
            let next = self.entries.range((lower, Bound::Unbounded)).next();
            let Some(((parent, seg), child)) = next else {
                stack.pop();
                continue;
            };
            if *parent != frame.oid || !seg.starts_with(&frame.filter) {
                stack.pop();
                continue;
            }
            frame.cursor = Some(seg.clone());
            let key = format!("{}{}", frame.prefix, String::from_utf8_lossy(seg));
            let folder = seg.ends_with(b"/");
            let child = child.clone();
            if folder && !delimited && self.has_children(&child) {
                // Descend unless the whole subtree sorts before `after`.
                if past(&key) || after.is_some_and(|a| a.starts_with(&key)) {
                    stack.push(Frame { oid: child, prefix: key, cursor: None, filter: Vec::new() });
                }
                continue;
            }
            if !past(&key) {
                continue;
            }
            out.push(if folder && delimited { Listed::Prefix { key } } else { Listed::Object { key, oid: child } });
        }
        let truncated = out.len() > max;
        out.truncate(max);
        (out, truncated)
    }
}

/// A drive's state as checkpoint tables (format §8.2), each sorted by its key.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Rows {
    pub entries: Vec<EntryRow>,
    pub objects: Vec<ObjectRecord>,
    pub history: Vec<HistoryRow>,
    pub removed: Vec<RemovedRow>,
}

/// What [`Rows::spill`] replaced: each descriptor with data extents, with the equivalent that
/// replaced it, and the shards that equivalent references.
#[derive(Clone, Debug, Default)]
pub struct Spilled {
    pub shards: Vec<Shard>,
    /// An object's current content.
    pub objects: Vec<(ObjectId, ContentDescriptor, ContentDescriptor)>,
    /// A version's content.
    pub versions: Vec<(VersionId, ContentDescriptor, ContentDescriptor)>,
}

impl Rows {
    /// Replaces every descriptor that has data extents with its equivalent in shards, as a
    /// checkpoint lists it (format §8.2).
    pub fn spill(&mut self) -> Spilled {
        let mut out = Spilled::default();
        let mut seen = std::collections::HashSet::new();
        let mut spill = |c: &mut Option<ContentDescriptor>, shards: &mut Vec<Shard>| {
            let from = c.as_ref().filter(|c| c.has_data())?.clone();
            let ContentDescriptor::Inline { extents } = &from else { return None };
            let s = crate::content::spill(extents);
            shards.extend(s.new_shards.into_iter().filter(|s| seen.insert(s.hash)));
            let to = ContentDescriptor::Inline { extents: s.extents };
            *c = Some(to.clone());
            Some((from, to))
        };
        for r in &mut self.objects {
            if let Some((from, to)) = spill(&mut r.content, &mut out.shards) {
                out.objects.push((r.oid.clone(), from, to));
            }
        }
        for r in &mut self.history {
            if let Some((from, to)) = spill(&mut r.content, &mut out.shards) {
                out.versions.push((r.version, from, to));
            }
        }
        out
    }
}

impl DriveState {
    /// Exports the state as checkpoint rows.
    pub fn rows(&self) -> Rows {
        let entries = self
            .entries
            .iter()
            .map(|((parent, _), oid)| {
                let loc = &self.locations[oid];
                EntryRow { parent: parent.clone(), name: loc.name.clone(), oid: oid.clone(), kind: self.kind(oid).unwrap_or(Kind::File) }
            })
            .collect();
        let mut objects: Vec<ObjectRecord> = self.objects.values().cloned().collect();
        objects.sort_by(|a, b| a.oid.cmp(&b.oid));
        let mut oids: Vec<&ObjectId> = self.history.keys().collect();
        oids.sort();
        let history = oids.into_iter().flat_map(|o| self.history[o].iter().cloned()).collect();
        let removed = self.removed.values().cloned().collect();
        Rows { entries, objects, history, removed }
    }

    /// Rebuilds a state from checkpoint rows taken at `seq`.
    pub fn from_rows(seq: u64, time: Option<Timestamp>, rows: Rows) -> Result<DriveState, StateError> {
        let mut s = DriveState { seq, time, ..DriveState::default() };
        for r in rows.objects {
            s.objects.insert(r.oid.clone(), r);
        }
        for e in rows.entries {
            names::validate_name(&e.name)?;
            if s.kind(&e.parent) != Some(Kind::Folder) {
                return Err(StateError::NotAFolder(e.parent));
            }
            if s.kind(&e.oid) != Some(e.kind) {
                return Err(StateError::UnknownObject(e.oid));
            }
            s.link(&e.oid, e.kind, &e.parent, &e.name);
        }
        for h in rows.history {
            s.versions.insert(h.version, h.oid.clone());
            s.history.entry(h.oid.clone()).or_default().push_back(h);
        }
        for (_, list) in s.history.iter_mut() {
            list.sort_by(|a, b| a.version.cmp(&b.version));
        }
        for r in rows.removed {
            s.removed_keys.insert(r.oid.clone(), r.key.clone());
            s.removed.insert((r.key.clone(), r.oid.clone()), r);
        }
        Ok(s)
    }

    /// This state with the equivalents [`Rows::spill`] made in place of the descriptors they
    /// replace, wherever those are still the same: once a checkpoint references the shards, the
    /// state need not hold their bytes. Its position is unchanged.
    pub fn with_spilled(&self, s: &Spilled) -> DriveState {
        let mut next = self.clone();
        for (oid, from, to) in &s.objects {
            if let Some(r) = next.objects.get_mut(oid)
                && r.content.as_ref() == Some(from)
            {
                r.content = Some(to.clone());
            }
        }
        for (v, from, to) in &s.versions {
            let Some(rows) = next.versions.get(v).and_then(|oid| next.history.get_mut(oid)) else { continue };
            if let Ok(i) = rows.binary_search_by(|r| r.version.cmp(v))
                && let Some(r) = rows.get_mut(i)
                && r.content.as_ref() == Some(from)
            {
                r.content = Some(to.clone());
            }
        }
        next
    }

    /// Total size of the files currently in the namespace.
    pub fn live_bytes(&self) -> u64 {
        self.locations.keys().filter_map(|o| self.objects.get(o)).filter(|r| r.kind == Kind::File).map(|r| r.size).sum()
    }

    /// Every object inside `folder`, parents before children: `(oid, parent, name, kind)`.
    pub fn subtree(&self, folder: &ObjectId) -> Vec<(ObjectId, ObjectId, String, Kind)> {
        let mut out = Vec::new();
        let mut stack = vec![folder.clone()];
        while let Some(f) = stack.pop() {
            let kids: Vec<_> = self.children(&f).map(|(_, o)| o.clone()).collect();
            for kid in kids.into_iter().rev() {
                let kind = self.kind(&kid).unwrap_or(Kind::File);
                let name = self.locations[&kid].name.clone();
                out.push((kid.clone(), f.clone(), name, kind));
                if kind == Kind::Folder {
                    stack.push(kid);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Actor, Attrs, Extent};
    use crate::ids::ShardHash;

    fn commit(seq: u64, txns: Vec<Txn>) -> Commit {
        Commit { format: 1, seq, time: Timestamp::now(), authority: "test".into(), txns }
    }

    fn txn(target: &ObjectId, op: Op, changes: Vec<Change>) -> Txn {
        Txn { target: target.clone(), op, actor: Actor::system(), changes }
    }

    fn create(oid: &ObjectId, parent: &ObjectId, name: &str, kind: Kind) -> Change {
        Change::Create(CreateChange { oid: oid.clone(), parent: parent.clone(), name: name.into(), kind })
    }

    fn content(n: u64) -> ContentDescriptor {
        ContentDescriptor::Inline { extents: vec![Extent::Shard { s: ShardHash::of(&n.to_be_bytes()), n }] }
    }

    fn set_content(oid: &ObjectId, n: u64) -> Change {
        let mut s = SetChange::new(oid.clone());
        s.content = Some(content(n));
        Change::Set(s)
    }

    /// Builds a drive with `a/`, `a/b.txt` (3 bytes), `a/c/` (empty) and `z.txt`.
    fn sample() -> (DriveState, [ObjectId; 4]) {
        let root = ObjectId::root();
        let [a, b, c, z] = [(); 4].map(|_| ObjectId::generate());
        let s = DriveState::empty()
            .apply(&commit(1, vec![
                txn(&b, Op::Put, vec![create(&a, &root, "a", Kind::Folder), create(&b, &a, "b.txt", Kind::File), set_content(&b, 3)]),
                txn(&c, Op::Put, vec![create(&c, &a, "c", Kind::Folder)]),
                txn(&z, Op::Put, vec![create(&z, &root, "z.txt", Kind::File), set_content(&z, 1)]),
            ]))
            .unwrap();
        (s, [a, b, c, z])
    }

    fn keys(items: &[Listed]) -> Vec<&str> {
        items.iter().map(Listed::key).collect()
    }

    #[test]
    fn lookups_and_keys_agree() {
        let (s, [a, b, c, z]) = sample();
        assert_eq!(s.lookup(&Key::parse("a/b.txt").unwrap()), Some(b.clone()));
        assert_eq!(s.lookup(&Key::parse("a/").unwrap()), Some(a.clone()));
        assert_eq!(s.lookup(&Key::parse("a").unwrap()), None, "a file key does not find a folder");
        assert_eq!(s.key_of(&c).as_deref(), Some("a/c/"));
        assert_eq!(s.key_of(&z).as_deref(), Some("z.txt"));
        assert_eq!(s.record(&b).unwrap().size, 3);
        assert_eq!(s.record(&b).unwrap().head, VersionId::new(1, 0));
        assert_eq!(s.version(&VersionId::new(1, 2)).unwrap().oid, z);
    }

    #[test]
    fn listings_follow_s3_rules() {
        let (s, _) = sample();
        assert_eq!(keys(&s.list("", false, None, 100).0), ["a/b.txt", "a/c/", "z.txt"]);
        assert_eq!(keys(&s.list("", true, None, 100).0), ["a/", "z.txt"]);
        assert_eq!(keys(&s.list("a/", true, None, 100).0), ["a/b.txt", "a/c/"]);
        assert_eq!(keys(&s.list("a/b", false, None, 100).0), ["a/b.txt"]);
        assert_eq!(keys(&s.list("a/c/", false, None, 100).0), ["a/c/"], "an empty folder lists itself");
        let (page, more) = s.list("", false, None, 2);
        assert!(more);
        assert_eq!(keys(&page), ["a/b.txt", "a/c/"]);
        let (page, more) = s.list("", false, Some("a/c/"), 2);
        assert!(!more);
        assert_eq!(keys(&page), ["z.txt"]);
        assert_eq!(keys(&s.list("", false, Some("a/"), 10).0), ["a/b.txt", "a/c/", "z.txt"]);
        assert!(s.list("nope/", false, None, 10).0.is_empty());
    }

    #[test]
    fn names_are_unique_across_kinds() {
        let (s, [a, ..]) = sample();
        let dup = ObjectId::generate();
        let r = s.apply(&commit(2, vec![txn(&dup, Op::Put, vec![create(&dup, &a, "c", Kind::File)])]));
        assert!(matches!(r, Err(StateError::NameTaken { .. })));
    }

    #[test]
    fn failed_commits_change_nothing() {
        let (s, [a, b, ..]) = sample();
        let fresh = ObjectId::generate();
        let bad = commit(2, vec![
            txn(&fresh, Op::Put, vec![create(&fresh, &a, "new.txt", Kind::File)]),
            txn(&b, Op::Put, vec![create(&b, &a, "again", Kind::File)]),
        ]);
        assert!(s.apply(&bad).is_err());
        assert_eq!(s.lookup(&Key::parse("a/new.txt").unwrap()), None);
        assert_eq!(s.seq(), 1);
        assert!(matches!(s.apply(&commit(3, vec![])), Err(StateError::BadSeq { expected: 2, got: 3 })));
    }

    #[test]
    fn a_commit_applies_a_transaction_at_a_time() {
        let (s, [a, b, ..]) = sample();
        let root = ObjectId::root();
        let t = s.time().unwrap();
        let moved = txn(&b, Op::Rename, vec![Change::Move(MoveChange { oid: b.clone(), parent: root, name: "b.txt".into() })]);
        let written = txn(&b, Op::Write, vec![set_content(&b, 7)]);
        let whole = s.apply(&Commit { time: t, ..commit(2, vec![moved.clone(), written.clone()]) }).unwrap();
        let first = s.apply_txn_of(2, 0, t, &moved).unwrap();
        assert_eq!((first.seq(), first.key_of(&b).as_deref(), first.record(&b).unwrap().size), (2, Some("b.txt"), 3));
        let second = first.apply_txn_of(2, 1, t, &written).unwrap();
        assert_eq!(second.rows(), whole.rows());
        assert_eq!(second.record(&b).unwrap().head, VersionId::new(2, 1));
        // Each step must follow the one before it, in the same commit.
        assert_eq!(s.apply_txn_of(2, 1, t, &written).unwrap_err(), StateError::BadSeq { expected: 1, got: 2 });
        assert_eq!(first.apply_txn_of(3, 1, t, &written).unwrap_err(), StateError::BadSeq { expected: 2, got: 3 });
        let later = Timestamp::from_datetime(t.datetime() + chrono::Duration::seconds(1));
        assert_eq!(first.apply_txn_of(2, 1, later, &written).unwrap_err(), StateError::TimeWentBackwards);
        let bad = txn(&a, Op::Put, vec![create(&a, &ObjectId::root(), "fresh", Kind::Folder)]);
        assert!(matches!(first.apply_txn_of(2, 1, t, &bad), Err(StateError::AlreadyLinked(_))));
        assert_eq!(first.record(&b).unwrap().size, 3, "a failed step changes nothing");
    }

    #[test]
    fn folders_move_with_their_subtree_but_not_into_themselves() {
        let (s, [a, b, c, _]) = sample();
        let root = ObjectId::root();
        let s2 = s
            .apply(&commit(2, vec![txn(&a, Op::Rename, vec![Change::Move(MoveChange { oid: a.clone(), parent: root.clone(), name: "moved".into() })])]))
            .unwrap();
        assert_eq!(s2.key_of(&b).as_deref(), Some("moved/b.txt"));
        assert_eq!(s2.lookup(&Key::parse("a/b.txt").unwrap()), None);
        let cyc = s2.apply(&commit(3, vec![txn(&a, Op::Rename, vec![Change::Move(MoveChange { oid: a.clone(), parent: c.clone(), name: "x".into() })])]));
        assert_eq!(cyc.unwrap_err(), StateError::Cycle(a));
    }

    #[test]
    fn removed_folders_come_back_whole() {
        let (s, [a, b, ..]) = sample();
        let root = ObjectId::root();
        let removed = s
            .apply(&commit(2, vec![txn(&a, Op::Delete, vec![Change::Remove(RemoveChange { oid: a.clone(), recursive: true })])]))
            .unwrap();
        assert_eq!(removed.lookup(&Key::parse("a/b.txt").unwrap()), None);
        let rows: Vec<_> = removed.removed("", None).collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, "a/");
        assert_eq!(rows[0].last_version, VersionId::new(1, 0));
        let back = removed.apply(&commit(3, vec![txn(&a, Op::Restore, vec![create(&a, &root, "a", Kind::Folder)])])).unwrap();
        assert_eq!(back.key_of(&b).as_deref(), Some("a/b.txt"));
        assert_eq!(back.removed("", None).count(), 0);
        let not_empty = s.apply(&commit(2, vec![txn(&a, Op::Delete, vec![Change::Remove(RemoveChange { oid: a.clone(), recursive: false })])]));
        assert_eq!(not_empty.unwrap_err(), StateError::FolderNotEmpty(a));
    }

    #[test]
    fn history_records_every_version_and_as_of_finds_them() {
        let (s, [_, b, ..]) = sample();
        let t1 = s.time().unwrap();
        let mut c2 = commit(2, vec![txn(&b, Op::Write, vec![set_content(&b, 9)])]);
        c2.time = Timestamp::from_datetime(t1.datetime() + chrono::Duration::seconds(5));
        let s2 = s.apply(&c2).unwrap();
        let mut attrs = SetChange::new(b.clone());
        attrs.attrs = Some(Attrs { mode: Some(0o600), ..Default::default() });
        let mut c3 = commit(3, vec![txn(&b, Op::Attrs, vec![Change::Set(attrs)])]);
        c3.time = c2.time;
        let s3 = s2.apply(&c3).unwrap();
        let h = s3.history(&b).unwrap();
        assert_eq!(h.iter().map(|r| r.op).collect::<Vec<_>>(), [Op::Put, Op::Write, Op::Attrs]);
        assert_eq!(h[2].etag, h[1].etag, "attributes do not change the ETag");
        assert_eq!(s3.as_of(&b, t1).unwrap().size, 3);
        assert_eq!(s3.as_of(&b, c2.time).unwrap().version, VersionId::new(3, 0));
        assert!(s3.as_of(&b, "2000-01-01T00:00:00Z".parse().unwrap()).is_none());
        let mut early = commit(4, vec![txn(&b, Op::Write, vec![set_content(&b, 1)])]);
        early.time = t1;
        assert_eq!(s3.apply(&early).unwrap_err(), StateError::TimeWentBackwards);
    }

    #[test]
    fn rows_round_trip() {
        let (s, [a, b, ..]) = sample();
        let s = s
            .apply(&commit(2, vec![txn(&b, Op::Delete, vec![Change::Remove(RemoveChange { oid: b.clone(), recursive: false })])]))
            .unwrap();
        let back = DriveState::from_rows(s.seq(), s.time(), s.rows()).unwrap();
        assert_eq!(back.rows(), s.rows());
        assert_eq!(back.key_of(&a).as_deref(), Some("a/"));
        assert_eq!(back.removed("", None).count(), 1);
        assert_eq!(back.version(&VersionId::new(1, 0)).unwrap().oid, b);
        assert_eq!(back.live_bytes(), 1);
        assert_eq!(s.subtree(&ObjectId::root()).len(), 3);
    }

    fn data(b: &'static [u8]) -> ContentDescriptor {
        ContentDescriptor::Inline { extents: vec![Extent::Data { d: bytes::Bytes::from_static(b) }] }
    }

    fn set_to(oid: &ObjectId, c: ContentDescriptor) -> Change {
        let mut s = SetChange::new(oid.clone());
        s.content = Some(c);
        Change::Set(s)
    }

    /// Rows as a checkpoint lists them: rows equal after this are the same bytes with the same
    /// ETags, whether held in data extents or in shards.
    fn spilled(rows: &Rows) -> Rows {
        let mut r = rows.clone();
        r.spill();
        r
    }

    /// Spilled rows hold no data extents and load as an equivalent state; the state in memory
    /// takes the equivalents only where it still has what was spilled.
    #[test]
    fn spilled_rows_are_equivalent_and_swap_in_where_unchanged() {
        let root = ObjectId::root();
        let [a, b] = [(); 2].map(|_| ObjectId::generate());
        let s = DriveState::empty()
            .apply(&commit(1, vec![
                txn(&a, Op::Put, vec![create(&a, &root, "a", Kind::File), set_to(&a, data(b"one"))]),
                txn(&b, Op::Put, vec![create(&b, &root, "b", Kind::File), set_to(&b, data(b"same"))]),
            ]))
            .unwrap()
            .apply(&commit(2, vec![txn(&a, Op::Write, vec![set_to(&a, data(b"two"))])]))
            .unwrap();
        let mut rows = s.rows();
        let spill = rows.spill();
        assert!(rows.objects.iter().all(|r| r.content.as_ref().is_none_or(|c| !c.has_data())));
        assert!(rows.history.iter().all(|r| r.content.as_ref().is_none_or(|c| !c.has_data())));
        assert_eq!((spill.objects.len(), spill.versions.len()), (2, 3));
        let mut hashes: Vec<_> = spill.shards.iter().map(|s| s.hash).collect();
        hashes.sort();
        let mut expected: Vec<_> = [&b"one"[..], b"two", b"same"].iter().map(|b| crate::ids::ShardHash::of(b)).collect();
        expected.sort();
        assert_eq!(hashes, expected, "each distinct content once");
        let loaded = DriveState::from_rows(s.seq(), s.time(), rows.clone()).unwrap();
        assert_ne!(loaded.rows(), s.rows());
        assert_eq!(spilled(&loaded.rows()), spilled(&s.rows()), "equivalent");
        assert_eq!(loaded.rows(), rows);
        for oid in [&a, &b] {
            assert_eq!(loaded.record(oid).unwrap().etag, s.record(oid).unwrap().etag);
        }
        // Meanwhile `b` changed and `a` was renamed: `b` keeps its new content, and the rename's
        // version, made after the spill, keeps what it copied.
        let later = s
            .apply(&commit(3, vec![
                txn(&b, Op::Write, vec![set_to(&b, data(b"new"))]),
                txn(&a, Op::Rename, vec![Change::Move(MoveChange { oid: a.clone(), parent: root.clone(), name: "a2".into() })]),
            ]))
            .unwrap();
        let swapped = later.with_spilled(&spill);
        assert_eq!(swapped.seq(), later.seq());
        assert_eq!(swapped.record(&b).unwrap().content, Some(data(b"new")));
        assert_eq!(swapped.record(&a).unwrap().content, rows.objects.iter().find(|r| r.oid == a).unwrap().content);
        assert_eq!(swapped.version(&VersionId::new(1, 1)).unwrap().content, rows.history.iter().find(|r| r.oid == b).unwrap().content);
        assert_eq!(swapped.version(&VersionId::new(3, 1)).unwrap().content, Some(data(b"two")));
        assert_eq!(spilled(&swapped.rows()), spilled(&later.rows()));
        assert_eq!(swapped.key_of(&a).as_deref(), Some("a2"));
    }

    #[test]
    fn sizes_must_match_content() {
        let (s, [_, b, ..]) = sample();
        let mut bad = SetChange::new(b.clone());
        bad.content = Some(content(4));
        bad.size = Some(5);
        let r = s.apply(&commit(2, vec![txn(&b, Op::Write, vec![Change::Set(bad)])]));
        assert_eq!(r.unwrap_err(), StateError::SizeMismatch { size: 5, content: 4 });
    }
}
