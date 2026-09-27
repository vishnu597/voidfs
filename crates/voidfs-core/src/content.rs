// SPDX-License-Identifier: Apache-2.0
//! File content as a flat list of extents, and the edits the protocol offers on it: offset
//! writes, splices (insert and remove) and resizes (protocol §4.1–§4.3).
//!
//! An edit re-chunks only the shards it touches and reuses every other shard, which is what
//! makes a 4 KiB change to a large file cost about one shard. Callers supply the bytes of the
//! shards an edit needs through a [`ShardSource`]; [`needed_shards`] says which those are.

use std::collections::VecDeque;

use bytes::{Bytes, BytesMut};

use crate::chunk::{self, Params, Shard};
use crate::ids::ShardHash;
use crate::model::Extent;

/// Supplies shard bytes during an edit.
pub trait ShardSource {
    fn shard(&mut self, hash: &ShardHash) -> Option<Bytes>;
}

impl<F: FnMut(&ShardHash) -> Option<Bytes>> ShardSource for F {
    fn shard(&mut self, hash: &ShardHash) -> Option<Bytes> {
        self(hash)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditError {
    #[error("shard {0} is needed for this edit but was not supplied")]
    MissingShard(ShardHash),
    #[error("the range {start}..{end} is outside the object's {size} bytes")]
    OutOfRange { start: u64, end: u64, size: u64 },
}

/// New content: its extents, and the shards it references that did not exist before.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Edited {
    pub extents: Vec<Extent>,
    pub new_shards: Vec<Shard>,
}

pub fn size(extents: &[Extent]) -> u64 {
    extents.iter().map(Extent::len).sum()
}

/// Merges adjacent zero runs and drops empty extents.
pub fn normalize(extents: impl IntoIterator<Item = Extent>) -> Vec<Extent> {
    let mut out: Vec<Extent> = Vec::new();
    for e in extents {
        if e.is_empty() {
            continue;
        }
        if let (Some(Extent::Zero { z }), Extent::Zero { z: more }) = (out.last_mut(), e) {
            *z += more;
            continue;
        }
        out.push(e);
    }
    out
}

/// The content of a whole new object.
pub fn from_bytes(data: &Bytes, p: Params) -> Edited {
    let new_shards = chunk::shards(data, p);
    let extents = new_shards.iter().map(|s| Extent::Shard { s: s.hash, n: s.bytes.len() as u64 }).collect();
    Edited { extents, new_shards }
}

/// Builds content from shards that were already cut (for example by a [`chunk::StreamChunker`]).
pub fn from_shards(shards: Vec<Shard>) -> Edited {
    let extents: Vec<_> = shards.iter().map(|s| Extent::Shard { s: s.hash, n: s.bytes.len() as u64 }).collect();
    Edited { extents: normalize(extents), new_shards: dedup(shards) }
}

/// One piece of a read: `len` bytes at `offset` within an extent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReadPiece {
    /// The shard to read from, or `None` for zeros.
    pub shard: Option<ShardHash>,
    pub offset: u64,
    pub len: u64,
}

/// Maps the byte range `start..start + len` onto the extents that hold it.
pub fn read_plan(extents: &[Extent], start: u64, len: u64) -> Result<Vec<ReadPiece>, EditError> {
    let total = size(extents);
    let end = start.checked_add(len).filter(|&e| e <= total).ok_or(EditError::OutOfRange {
        start,
        end: start.saturating_add(len),
        size: total,
    })?;
    let mut out = Vec::new();
    let mut pos = 0;
    for e in extents {
        let (e_start, e_end) = (pos, pos + e.len());
        pos = e_end;
        if e_end <= start {
            continue;
        }
        if e_start >= end {
            break;
        }
        let from = start.max(e_start);
        let to = end.min(e_end);
        out.push(ReadPiece { shard: e.shard(), offset: from - e_start, len: to - from });
    }
    Ok(out)
}

/// The shards an edit of `start..end` may read: those overlapping it, plus neighbors used to
/// avoid leaving tiny shards behind. Fetching these before [`splice`] means it never misses.
pub fn needed_shards(extents: &[Extent], start: u64, end: u64) -> Vec<ShardHash> {
    // `lo`: the first extent ending after `start`. `hi`: one past the last extent starting
    // before `end`. Extents in `lo..hi` are cut by the edit; `lo - 1` may be absorbed in front
    // and up to MAX_PULLS extents from `hi` behind.
    let mut lo = extents.len();
    let mut hi = 0;
    let mut pos = 0;
    for (i, e) in extents.iter().enumerate() {
        let (e_start, e_end) = (pos, pos + e.len());
        pos = e_end;
        if e_end > start && lo == extents.len() {
            lo = i;
        }
        if e_start < end {
            hi = i + 1;
        }
    }
    let from = lo.saturating_sub(1);
    let to = (hi.max(lo) + MAX_PULLS).min(extents.len());
    let mut out: Vec<ShardHash> = Vec::new();
    for e in extents.get(from..to).unwrap_or(&[]) {
        if let Some(h) = e.shard()
            && !out.contains(&h) {
                out.push(h);
            }
    }
    out
}

/// How many following shards an edit may absorb to avoid a tiny trailing shard.
const MAX_PULLS: usize = 2;

/// Replaces `remove` bytes at `start` with `insert` (protocol §4.3). The general edit that
/// writes, inserts, deletes and truncates are built from.
pub fn splice(
    extents: &[Extent],
    start: u64,
    remove: u64,
    insert: &[u8],
    p: Params,
    src: &mut impl ShardSource,
) -> Result<Edited, EditError> {
    let total = size(extents);
    let end = start
        .checked_add(remove)
        .filter(|&e| e <= total)
        .ok_or(EditError::OutOfRange { start, end: start.saturating_add(remove), size: total })?;

    let mut before: Vec<Extent> = Vec::new();
    let mut after: VecDeque<Extent> = VecDeque::new();
    let mut front = BytesMut::new(); // kept bytes of a shard cut by `start`
    let mut back = Bytes::new(); // kept bytes of a shard cut by `end`
    let mut back_zero = 0u64; // kept zeros of a zero run cut by `end`

    let mut pos = 0;
    for e in extents {
        let (e_start, e_end) = (pos, pos + e.len());
        pos = e_end;
        if e_end <= start {
            before.push(*e);
            continue;
        }
        if e_start >= end {
            after.push_back(*e);
            continue;
        }
        // `e` overlaps the edited range: keep what lies outside it.
        let keep_left = start.saturating_sub(e_start).min(e.len());
        let keep_right = e_end.saturating_sub(end.max(e_start));
        match *e {
            Extent::Zero { .. } => {
                if keep_left > 0 {
                    before.push(Extent::Zero { z: keep_left });
                }
                back_zero += keep_right;
            }
            Extent::Shard { s, .. } => {
                let bytes = src.shard(&s).ok_or(EditError::MissingShard(s))?;
                if keep_left > 0 {
                    front.extend_from_slice(&bytes[..keep_left as usize]);
                }
                if keep_right > 0 {
                    back = bytes.slice(bytes.len() - keep_right as usize..);
                }
            }
        }
    }
    if back_zero > 0 {
        after.push_front(Extent::Zero { z: back_zero });
    }

    let mut region = front;
    region.extend_from_slice(insert);
    region.extend_from_slice(&back);

    // A short region would become a tiny shard: absorb the previous shard, whose start is a
    // chunk boundary, so the region is re-chunked from a boundary.
    if !region.is_empty() && region.len() < p.min
        && let Some(Extent::Shard { s, .. }) = before.last().copied()
            && let Some(bytes) = src.shard(&s) {
                before.pop();
                let mut joined = BytesMut::from(&bytes[..]);
                joined.extend_from_slice(&region);
                region = joined;
            }

    let mut region = region.freeze();
    let mut new_shards = chunk::shards(&region, p);
    // Likewise absorb following shards while the last new shard is shorter than the minimum.
    for _ in 0..MAX_PULLS {
        let short = new_shards.last().is_some_and(|s| s.bytes.len() < p.min);
        let Some(Extent::Shard { s, .. }) = after.front().copied() else { break };
        if !short {
            break;
        }
        let Some(bytes) = src.shard(&s) else { break };
        after.pop_front();
        let mut joined = BytesMut::from(&region[..]);
        joined.extend_from_slice(&bytes);
        region = joined.freeze();
        new_shards = chunk::shards(&region, p);
    }

    let mid = new_shards.iter().map(|s| Extent::Shard { s: s.hash, n: s.bytes.len() as u64 });
    let extents = normalize(before.into_iter().chain(mid).chain(after));
    Ok(Edited { extents, new_shards: dedup(new_shards) })
}

/// `pwrite`: writes `data` at `offset`, extending with zeros if `offset` is past the end
/// (protocol §4.1).
pub fn write_at(
    extents: &[Extent],
    offset: u64,
    data: &[u8],
    p: Params,
    src: &mut impl ShardSource,
) -> Result<Edited, EditError> {
    let total = size(extents);
    let mut base = extents.to_vec();
    if offset > total {
        base.push(Extent::Zero { z: offset - total });
    }
    let base = normalize(base);
    let current = total.max(offset);
    let remove = (data.len() as u64).min(current - offset);
    splice(&base, offset, remove, data, p, src)
}

/// Truncates or extends with zeros to exactly `new_size` bytes.
pub fn set_size(extents: &[Extent], new_size: u64, p: Params, src: &mut impl ShardSource) -> Result<Edited, EditError> {
    let total = size(extents);
    if new_size >= total {
        let mut out = extents.to_vec();
        out.push(Extent::Zero { z: new_size - total });
        return Ok(Edited { extents: normalize(out), new_shards: Vec::new() });
    }
    splice(extents, new_size, total - new_size, &[], p, src)
}

/// Applies several edits in order, returning only the new shards the final content uses.
pub fn apply_edits(
    extents: &[Extent],
    edits: &[crate::patch::Edit<'_>],
    p: Params,
    src: &mut impl ShardSource,
) -> Result<Edited, EditError> {
    let mut current = Edited { extents: extents.to_vec(), new_shards: Vec::new() };
    let mut made: std::collections::HashMap<ShardHash, Bytes> = std::collections::HashMap::new();
    for edit in edits {
        let mut both = |h: &ShardHash| made.get(h).cloned().or_else(|| src.shard(h));
        let next = write_at(&current.extents, edit.offset, edit.data, p, &mut both)?;
        for s in next.new_shards {
            made.insert(s.hash, s.bytes);
        }
        current.extents = next.extents;
    }
    let used: std::collections::HashSet<_> = current.extents.iter().filter_map(Extent::shard).collect();
    current.new_shards = made
        .into_iter()
        .filter(|(h, _)| used.contains(h))
        .map(|(hash, bytes)| Shard { hash, bytes })
        .collect();
    Ok(current)
}

fn dedup(shards: Vec<Shard>) -> Vec<Shard> {
    let mut seen = std::collections::HashSet::new();
    shards.into_iter().filter(|s| seen.insert(s.hash)).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use proptest::prelude::*;

    use super::*;
    use crate::chunk::tests::{SMALL, random_bytes};

    /// An in-memory shard store and the object's expected bytes, kept side by side.
    struct Model {
        store: HashMap<ShardHash, Bytes>,
        extents: Vec<Extent>,
        bytes: Vec<u8>,
    }

    impl Model {
        fn new(data: Vec<u8>) -> Self {
            let mut m = Model { store: HashMap::new(), extents: Vec::new(), bytes: data.clone() };
            let e = from_bytes(&Bytes::from(data), SMALL);
            m.absorb(e);
            m
        }

        fn absorb(&mut self, e: Edited) {
            for s in e.new_shards {
                assert_eq!(ShardHash::of(&s.bytes), s.hash);
                self.store.insert(s.hash, s.bytes);
            }
            self.extents = e.extents;
        }

        /// A source that only has the shards `needed_shards` names, to prove that list is enough.
        fn narrow_source(&self, start: u64, end: u64) -> HashMap<ShardHash, Bytes> {
            needed_shards(&self.extents, start, end).into_iter().map(|h| (h, self.store[&h].clone())).collect()
        }

        fn materialize(&self) -> Vec<u8> {
            let mut out = Vec::new();
            for e in &self.extents {
                match e {
                    Extent::Shard { s, n } => {
                        let b = &self.store[s];
                        assert_eq!(b.len() as u64, *n);
                        out.extend_from_slice(b);
                    }
                    Extent::Zero { z } => out.resize(out.len() + *z as usize, 0),
                }
            }
            out
        }

        fn check(&self) {
            assert_eq!(self.materialize(), self.bytes);
            assert_eq!(size(&self.extents), self.bytes.len() as u64);
            assert_eq!(normalize(self.extents.clone()), self.extents, "extents are normalized");
            for e in &self.extents {
                if let Extent::Shard { n, .. } = e {
                    assert!(*n >= 1 && *n <= SMALL.max as u64);
                }
            }
        }
    }

    #[derive(Debug, Clone)]
    enum Op {
        Write(u64, Vec<u8>),
        Splice(u64, u64, Vec<u8>),
        Resize(u64),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u64..12_000, prop::collection::vec(any::<u8>(), 0..3000)).prop_map(|(o, d)| Op::Write(o, d)),
            (0u64..12_000, 0u64..3000, prop::collection::vec(any::<u8>(), 0..3000)).prop_map(|(o, r, d)| Op::Splice(o, r, d)),
            (0u64..14_000).prop_map(Op::Resize),
        ]
    }

    proptest! {
        #[test]
        fn edits_match_a_byte_buffer(seed in 0u64..1000, len in 0usize..10_000, ops in prop::collection::vec(op(), 1..12)) {
            let mut m = Model::new(random_bytes(seed, len));
            m.check();
            for op in ops {
                let size = m.bytes.len() as u64;
                match op {
                    Op::Write(o, d) => {
                        let mut src = m.narrow_source(o.min(size), o + d.len() as u64);
                        let e = write_at(&m.extents, o, &d, SMALL, &mut |h: &ShardHash| src.remove(h)).unwrap();
                        m.absorb(e);
                        let o = o as usize;
                        if o > m.bytes.len() { m.bytes.resize(o, 0); }
                        let end = (o + d.len()).min(m.bytes.len());
                        m.bytes.splice(o..end, d);
                    }
                    Op::Splice(o, r, d) => {
                        let (o, r) = (o.min(size), r.min(size - o.min(size)));
                        let mut src = m.narrow_source(o, o + r);
                        let e = splice(&m.extents, o, r, &d, SMALL, &mut |h: &ShardHash| src.remove(h)).unwrap();
                        m.absorb(e);
                        m.bytes.splice(o as usize..(o + r) as usize, d);
                    }
                    Op::Resize(n) => {
                        let mut src = m.narrow_source(n.min(size), size);
                        let e = set_size(&m.extents, n, SMALL, &mut |h: &ShardHash| src.remove(h)).unwrap();
                        m.absorb(e);
                        m.bytes.resize(n as usize, 0);
                    }
                }
                m.check();
            }
        }
    }

    #[test]
    fn a_small_edit_to_a_large_file_touches_few_shards() {
        let mut m = Model::new(random_bytes(9, 200_000));
        let before: std::collections::HashSet<_> = m.extents.iter().filter_map(Extent::shard).collect();
        let e = write_at(&m.extents, 100_000, b"four", SMALL, &mut |h: &ShardHash| m.store.get(h).cloned()).unwrap();
        assert!(e.new_shards.len() <= 3, "{} new shards", e.new_shards.len());
        let e = {
            let n = e.new_shards.len();
            m.absorb(e);
            n
        };
        let after: std::collections::HashSet<_> = m.extents.iter().filter_map(Extent::shard).collect();
        assert!(after.difference(&before).count() <= e);
        m.bytes[100_000..100_004].copy_from_slice(b"four");
        m.check();
    }

    #[test]
    fn extending_never_stores_zeros() {
        let m = Model::new(b"ab".to_vec());
        let e = set_size(&m.extents, 1 << 40, SMALL, &mut |_: &ShardHash| None).unwrap();
        assert!(e.new_shards.is_empty());
        assert_eq!(e.extents.last(), Some(&Extent::Zero { z: (1 << 40) - 2 }));
        // Writing into the middle of a huge zero run only stores the written bytes.
        let e2 = write_at(&e.extents, 1 << 39, b"x", SMALL, &mut |_: &ShardHash| None).unwrap();
        assert_eq!(e2.new_shards.len(), 1);
        assert_eq!(size(&e2.extents), 1 << 40);
    }

    #[test]
    fn out_of_range_edits_are_refused() {
        let m = Model::new(b"hello".to_vec());
        let r = splice(&m.extents, 3, 10, b"", SMALL, &mut |h: &ShardHash| m.store.get(h).cloned());
        assert_eq!(r, Err(EditError::OutOfRange { start: 3, end: 13, size: 5 }));
    }

    #[test]
    fn read_plans_cover_exactly_the_range() {
        let m = Model::new(random_bytes(4, 5000));
        let plan = read_plan(&m.extents, 1000, 2500).unwrap();
        let mut got = Vec::new();
        for p in plan {
            let b = &m.store[&p.shard.unwrap()];
            got.extend_from_slice(&b[p.offset as usize..(p.offset + p.len) as usize]);
        }
        assert_eq!(got, &m.bytes[1000..3500]);
        assert!(read_plan(&m.extents, 4000, 1001).is_err());
    }

    #[test]
    fn patches_keep_only_final_shards() {
        let m = Model::new(b"hello world".to_vec());
        let edits = [crate::patch::Edit { offset: 0, data: b"aaaa" }, crate::patch::Edit { offset: 2, data: b"ZZ" }];
        let e = apply_edits(&m.extents, &edits, SMALL, &mut |h: &ShardHash| m.store.get(h).cloned()).unwrap();
        let used: std::collections::HashSet<_> = e.extents.iter().filter_map(Extent::shard).collect();
        assert!(e.new_shards.iter().all(|s| used.contains(&s.hash)));
        let bytes: Vec<u8> = e.new_shards.iter().flat_map(|s| s.bytes.to_vec()).collect();
        assert_eq!(bytes, b"aaZZo world");
    }
}
