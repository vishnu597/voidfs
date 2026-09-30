// SPDX-License-Identifier: Apache-2.0
//! File content as a flat list of extents, and the edits the protocol offers on it: offset
//! writes, splices (insert and remove) and resizes (protocol §4.1–§4.3).
//!
//! An edit re-chunks only the shards it touches and reuses every other shard, which is what
//! makes a 4 KiB change to a large file cost about one shard. Callers supply the bytes of the
//! shards an edit needs through a [`ShardSource`]; [`needed_shards`] says which those are.
//!
//! Data extents (format §5) are treated as shards whose bytes are in hand: an edit that touches
//! one, or leaves a short region beside it, re-chunks its bytes into shards, and one it does not
//! touch is kept. Whether a result is held in data extents is the writer's choice ([`inline`],
//! [`spill`]).

use std::collections::VecDeque;

use bytes::{Bytes, BytesMut};

use crate::chunk::{self, Params, Shard};
use crate::ids::ShardHash;
use crate::model::{Extent, MAX_DATA_BYTES};

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
        if let (Some(Extent::Zero { z }), Extent::Zero { z: more }) = (out.last_mut(), &e) {
            *z += more;
            continue;
        }
        out.push(e);
    }
    out
}

/// Content held in its descriptor: one data extent, or none for the empty file (format §5).
/// `data` is at most [`MAX_DATA_BYTES`], and is copied, so that it does not keep a larger buffer
/// it may be a slice of alive.
pub fn inline(data: &[u8]) -> Vec<Extent> {
    assert!(data.len() <= MAX_DATA_BYTES, "{} bytes cannot be held in a descriptor", data.len());
    if data.is_empty() { Vec::new() } else { vec![Extent::Data { d: Bytes::copy_from_slice(data) }] }
}

/// Replaces each data extent with the shard extent of the same bytes (format §8.2), and returns
/// those shards, which the result references and which may not be stored yet.
pub fn spill(extents: &[Extent]) -> Edited {
    let mut new_shards = Vec::new();
    let extents = extents
        .iter()
        .map(|e| match e {
            Extent::Data { d } => {
                let s = Shard::new(d.clone());
                let e = Extent::Shard { s: s.hash, n: s.bytes.len() as u64 };
                new_shards.push(s);
                e
            }
            e => e.clone(),
        })
        .collect();
    Edited { extents, new_shards: dedup(new_shards) }
}

/// The bytes of content, with its shards' bytes from `src`. For small content only: it is all
/// in memory at once.
pub fn materialize(extents: &[Extent], src: &mut impl ShardSource) -> Result<Bytes, EditError> {
    let mut out = BytesMut::with_capacity(size(extents) as usize);
    for e in extents {
        match e {
            Extent::Shard { s, .. } => out.extend_from_slice(&src.shard(s).ok_or(EditError::MissingShard(*s))?),
            Extent::Zero { z } => out.resize(out.len() + *z as usize, 0),
            Extent::Data { d } => out.extend_from_slice(d),
        }
    }
    Ok(out.freeze())
}

/// The bytes of an extent that holds some: a shard's from `src`, if it has them, or a data
/// extent's own.
fn bytes_of(e: &Extent, src: &mut impl ShardSource) -> Option<Bytes> {
    match e {
        Extent::Shard { s, .. } => src.shard(s),
        Extent::Data { d } => Some(d.clone()),
        Extent::Zero { .. } => None,
    }
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

/// Where a piece of a read comes from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Source {
    Shard(ShardHash),
    Zeros,
    /// Bytes held in the descriptor, already cut to the piece.
    Data(Bytes),
}

/// One piece of a read: `len` bytes at `offset` within an extent.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReadPiece {
    pub source: Source,
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
        let (offset, len) = (from - e_start, to - from);
        let source = match e {
            Extent::Shard { s, .. } => Source::Shard(*s),
            Extent::Zero { .. } => Source::Zeros,
            Extent::Data { d } => Source::Data(d.slice(offset as usize..(offset + len) as usize)),
        };
        out.push(ReadPiece { source, offset, len });
    }
    Ok(out)
}

/// The shards an edit of `start..end` may read: those overlapping it, plus neighbors used to
/// avoid leaving tiny shards behind. Fetching these before [`splice`] means it never misses.
pub fn needed_shards(extents: &[Extent], start: u64, end: u64) -> Vec<ShardHash> {
    let mut out: Vec<ShardHash> = Vec::new();
    for e in extents.get(neighbourhood(extents, start, end)).unwrap_or(&[]) {
        if let Some(h) = e.shard()
            && !out.contains(&h) {
                out.push(h);
            }
    }
    out
}

/// The extents an edit of `start..end` may read, by index.
fn neighbourhood(extents: &[Extent], start: u64, end: u64) -> std::ops::Range<usize> {
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
    lo.saturating_sub(1)..(hi.max(lo) + MAX_PULLS).min(extents.len())
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
            before.push(e.clone());
            continue;
        }
        if e_start >= end {
            after.push_back(e.clone());
            continue;
        }
        // `e` overlaps the edited range: keep what lies outside it.
        let keep_left = start.saturating_sub(e_start).min(e.len());
        let keep_right = e_end.saturating_sub(end.max(e_start));
        let bytes = match e {
            Extent::Zero { .. } => {
                if keep_left > 0 {
                    before.push(Extent::Zero { z: keep_left });
                }
                back_zero += keep_right;
                continue;
            }
            Extent::Shard { s, .. } => src.shard(s).ok_or(EditError::MissingShard(*s))?,
            Extent::Data { d } => d.clone(),
        };
        if keep_left > 0 {
            front.extend_from_slice(&bytes[..keep_left as usize]);
        }
        if keep_right > 0 {
            back = bytes.slice(bytes.len() - keep_right as usize..);
        }
    }
    if back_zero > 0 {
        after.push_front(Extent::Zero { z: back_zero });
    }

    let mut region = front;
    region.extend_from_slice(insert);
    region.extend_from_slice(&back);

    // A short region would become a tiny shard: absorb the previous shard, whose start is a
    // chunk boundary, so the region is re-chunked from a boundary. A data extent is absorbed
    // likewise, so that a new shard is not left beside it.
    if !region.is_empty() && region.len() < p.min
        && let Some(bytes) = before.last().and_then(|e| bytes_of(e, src)) {
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
        if !short {
            break;
        }
        let Some(bytes) = after.front().and_then(|e| bytes_of(e, src)) else { break };
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

/// Applies several writes in order, later ones winning where they overlap (protocol §4.2), and
/// extends with zeros to the furthest edit's end.
///
/// Edits that share bytes of a shard, a data extent or a zero run are one group: every edit in
/// it is applied to those bytes, and they are re-chunked once, with what [`splice`] does at the
/// edges (a short group absorbs the extent before it; a short last shard pulls in up to
/// [`MAX_PULLS`] extents after it, or the next group whole). So each touched shard is chunked and
/// hashed once, however many edits land in it. Groups that only meet at a boundary stay apart,
/// as they would edit by edit, each chunked up to its own end. [`needed_shards`] over each edit's
/// range names every shard this reads.
pub fn apply_edits(
    extents: &[Extent],
    edits: &[crate::patch::Edit<'_>],
    p: Params,
    src: &mut impl ShardSource,
) -> Result<Edited, EditError> {
    let total = size(extents);
    let end = crate::patch::default_size(total, edits);
    let mut base = extents.to_vec();
    if end > total {
        base.push(Extent::Zero { z: end - total });
    }
    let base = normalize(base);
    let mut starts = Vec::with_capacity(base.len() + 1);
    let mut pos = 0;
    for e in &base {
        starts.push(pos);
        pos += e.len();
    }
    starts.push(pos);
    let holding = |at: u64| starts.partition_point(|&s| s <= at) - 1;
    let edits: Vec<_> = edits.iter().filter(|e| !e.data.is_empty()).collect();

    // Each edit's reach: its range, widened to the whole shards and data extents it touches. A
    // zero run is cut, not re-chunked, so only the written bytes of one join a group.
    let mut reach: Vec<(u64, u64)> = edits
        .iter()
        .map(|e| {
            let (a, b) = (e.offset, e.offset + e.data.len() as u64);
            let (i, j) = (holding(a), holding(b - 1));
            let a = if matches!(base[i], Extent::Zero { .. }) { a } else { starts[i] };
            let b = if matches!(base[j], Extent::Zero { .. }) { b } else { starts[j + 1] };
            (a, b)
        })
        .collect();
    reach.sort_unstable();
    let mut groups: Vec<(u64, u64)> = Vec::new();
    for (a, b) in reach {
        match groups.last_mut() {
            Some(g) if a < g.1 => g.1 = g.1.max(b),
            _ => groups.push((a, b)),
        }
    }

    // Each group's bytes, with its edits applied in order.
    let mut bytes = Vec::with_capacity(groups.len());
    for &(a, b) in &groups {
        let mut buf = BytesMut::with_capacity((b - a) as usize);
        for e in clip(&base, &starts, a, b) {
            match e {
                Extent::Zero { z } => buf.resize(buf.len() + z as usize, 0),
                e => buf.extend_from_slice(&bytes_of(&e, src).ok_or_else(|| EditError::MissingShard(e.shard().unwrap()))?),
            }
        }
        bytes.push(buf);
    }
    for e in &edits {
        let g = groups.partition_point(|g| g.0 <= e.offset) - 1;
        let at = (e.offset - groups[g].0) as usize;
        bytes[g][at..at + e.data.len()].copy_from_slice(e.data);
    }

    // The content in order: extents kept as they are, and the groups between them.
    let mut segments: VecDeque<Segment> = VecDeque::new();
    let mut at = 0;
    for (&(a, b), buf) in groups.iter().zip(bytes) {
        segments.extend(clip(&base, &starts, at, a).map(Segment::Kept));
        segments.push_back(Segment::Group(buf));
        at = b;
    }
    segments.extend(clip(&base, &starts, at, end).map(Segment::Kept));

    let mut out: Vec<Segment> = Vec::new();
    while let Some(s) = segments.pop_front() {
        let Segment::Group(mut region) = s else {
            out.push(s);
            continue;
        };
        if region.len() < p.min
            && let Some(prev) = out.pop() {
                match segment_bytes(&prev, src)? {
                    Some(b) => {
                        let mut joined = BytesMut::with_capacity(b.len() + region.len());
                        joined.extend_from_slice(&b);
                        joined.extend_from_slice(&region);
                        region = joined;
                    }
                    None => out.push(prev),
                }
            }
        // A cut before the last is final however much follows (chunk.rs), so each pull re-cuts
        // only from the start of the last, short, shard.
        let mut lens = chunk::cut(&region, p);
        let mut pulls = 0;
        while let Some(&last) = lens.last()
            && last < p.min
        {
            // The next group is pulled in whole, and may then pull extents after it itself.
            match segments.front() {
                Some(Segment::Group(_)) => pulls = 0,
                Some(Segment::Kept(e)) if pulls < MAX_PULLS && !matches!(e, Extent::Zero { .. }) => pulls += 1,
                _ => break,
            }
            let from = region.len() - last;
            match segments.pop_front().unwrap() {
                Segment::Group(b) => region.extend_from_slice(&b),
                next => region.extend_from_slice(&segment_bytes(&next, src)?.unwrap()),
            }
            lens.pop();
            lens.extend(chunk::cut(&region[from..], p));
        }
        let region = region.freeze();
        let mut from = 0;
        for len in lens {
            out.push(Segment::Cut(region.slice(from..from + len)));
            from += len;
        }
    }

    let mut new_shards = Vec::new();
    let extents = out.into_iter().map(|s| match s {
        Segment::Kept(e) => e,
        Segment::Cut(b) => {
            let s = Shard::new(b);
            let e = Extent::Shard { s: s.hash, n: s.bytes.len() as u64 };
            new_shards.push(s);
            e
        }
        Segment::Group(_) => unreachable!("every group is cut"),
    });
    let extents = normalize(extents.collect::<Vec<_>>());
    Ok(Edited { extents, new_shards: dedup(new_shards) })
}

/// A piece of content while [`apply_edits`] rebuilds it.
enum Segment {
    /// An extent kept as it is.
    Kept(Extent),
    /// The bytes of a group of edits, not yet chunked.
    Group(BytesMut),
    /// A new shard, not yet hashed.
    Cut(Bytes),
}

/// The bytes a kept extent or a new shard holds: `None` for zeros, and an error for a shard `src`
/// lacks.
fn segment_bytes(s: &Segment, src: &mut impl ShardSource) -> Result<Option<Bytes>, EditError> {
    match s {
        Segment::Kept(Extent::Zero { .. }) => Ok(None),
        Segment::Kept(e) => bytes_of(e, src).map(Some).ok_or_else(|| EditError::MissingShard(e.shard().unwrap())),
        Segment::Cut(b) => Ok(Some(b.clone())),
        Segment::Group(_) => unreachable!("a group is taken whole"),
    }
}

/// The extents of `base` (which begin at `starts`) within `from..to`, where only zero runs may be
/// cut.
fn clip<'a>(base: &'a [Extent], starts: &'a [u64], from: u64, to: u64) -> impl Iterator<Item = Extent> + 'a {
    let first = starts.partition_point(|&s| s <= from).saturating_sub(1);
    base[first..].iter().zip(&starts[first..]).take_while(move |&(_, &s)| s < to).filter_map(move |(e, &s)| {
        let (a, b) = (s.max(from), (s + e.len()).min(to));
        if a >= b {
            return None;
        }
        match e {
            Extent::Zero { .. } => Some(Extent::Zero { z: b - a }),
            e => {
                debug_assert_eq!((a, b), (s, s + e.len()), "only a zero run is cut");
                Some(e.clone())
            }
        }
    })
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
    use crate::model::ContentDescriptor;

    /// An in-memory shard store and the object's expected bytes, kept side by side.
    struct Model {
        store: HashMap<ShardHash, Bytes>,
        extents: Vec<Extent>,
        bytes: Vec<u8>,
        /// The most bytes data extents held at the start; edits never add any.
        data: usize,
    }

    /// A run of content to start from: bytes cut into shards, a data extent, or zeros.
    #[derive(Debug, Clone)]
    enum Piece {
        Shards(Vec<u8>),
        Data(Vec<u8>),
        Zeros(u64),
    }

    fn piece() -> impl Strategy<Value = Piece> {
        prop_oneof![
            (0u64..1000, 1usize..3000).prop_map(|(seed, len)| Piece::Shards(random_bytes(seed, len))),
            prop::collection::vec(any::<u8>(), 1..600).prop_map(Piece::Data),
            (1u64..2000).prop_map(Piece::Zeros),
        ]
    }

    impl Model {
        fn new(data: Vec<u8>) -> Self {
            Model::of(vec![Piece::Shards(data)])
        }

        fn of(pieces: Vec<Piece>) -> Self {
            let mut m = Model { store: HashMap::new(), extents: Vec::new(), bytes: Vec::new(), data: 0 };
            let mut extents = Vec::new();
            for p in pieces {
                match p {
                    Piece::Shards(b) => {
                        let e = from_bytes(&Bytes::from(b.clone()), SMALL);
                        for s in e.new_shards {
                            m.store.insert(s.hash, s.bytes);
                        }
                        extents.extend(e.extents);
                        m.bytes.extend(b);
                    }
                    Piece::Data(b) => {
                        m.bytes.extend(&b);
                        extents.push(Extent::Data { d: Bytes::from(b) });
                    }
                    Piece::Zeros(z) => {
                        m.bytes.resize(m.bytes.len() + z as usize, 0);
                        extents.push(Extent::Zero { z });
                    }
                }
            }
            m.extents = normalize(extents);
            m.data = crate::model::data_len(&m.extents);
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
                    Extent::Data { d } => out.extend_from_slice(d),
                }
            }
            out
        }

        fn check(&self) {
            assert_eq!(self.materialize(), self.bytes);
            assert_eq!(materialize(&self.extents, &mut |h: &ShardHash| self.store.get(h).cloned()).unwrap(), self.bytes);
            assert_eq!(size(&self.extents), self.bytes.len() as u64);
            assert_eq!(normalize(self.extents.clone()), self.extents, "extents are normalized");
            for e in &self.extents {
                if let Extent::Shard { n, .. } = e {
                    assert!(*n >= 1 && *n <= SMALL.max as u64);
                }
            }
            assert!(crate::model::data_len(&self.extents) <= self.data, "an edit adds no data extents");
            // Spilled, the content is the same bytes with the same ETag, and no data extents.
            let spilled = spill(&self.extents);
            let mut store = self.store.clone();
            store.extend(spilled.new_shards.iter().map(|s| (s.hash, s.bytes.clone())));
            assert_eq!(crate::model::data_len(&spilled.extents), 0);
            assert_eq!(materialize(&spilled.extents, &mut |h: &ShardHash| store.get(h).cloned()).unwrap(), self.bytes);
            let desc = |extents: &[Extent]| ContentDescriptor::Inline { extents: extents.to_vec() };
            assert_eq!(desc(&spilled.extents).etag(), desc(&self.extents).etag());
        }

        /// Applies `ops` one after another, checking the content after each.
        fn run(mut self, ops: Vec<Op>) {
            self.check();
            for op in ops {
                let size = self.bytes.len() as u64;
                match op {
                    Op::Write(o, d) => {
                        let src = self.narrow_source(o.min(size), o + d.len() as u64);
                        let e = write_at(&self.extents, o, &d, SMALL, &mut |h: &ShardHash| src.get(h).cloned()).unwrap();
                        self.absorb(e);
                        let o = o as usize;
                        if o > self.bytes.len() { self.bytes.resize(o, 0); }
                        let end = (o + d.len()).min(self.bytes.len());
                        self.bytes.splice(o..end, d);
                    }
                    Op::Splice(o, r, d) => {
                        let (o, r) = (o.min(size), r.min(size - o.min(size)));
                        let src = self.narrow_source(o, o + r);
                        let e = splice(&self.extents, o, r, &d, SMALL, &mut |h: &ShardHash| src.get(h).cloned()).unwrap();
                        self.absorb(e);
                        self.bytes.splice(o as usize..(o + r) as usize, d);
                    }
                    Op::Resize(n) => {
                        let src = self.narrow_source(n.min(size), size);
                        let e = set_size(&self.extents, n, SMALL, &mut |h: &ShardHash| src.get(h).cloned()).unwrap();
                        self.absorb(e);
                        self.bytes.resize(n as usize, 0);
                    }
                    Op::Patch(edits) => {
                        self.patch(&edits);
                    }
                }
                self.check();
            }
        }

        /// A source with the shards `needed_shards` names for each edit's range, as the server
        /// fetches them for a patch.
        fn patch_source(&self, edits: &[(u64, Vec<u8>)]) -> HashMap<ShardHash, Bytes> {
            let size = self.bytes.len() as u64;
            let mut src = HashMap::new();
            for (o, d) in edits {
                let a = (*o).min(size);
                src.extend(self.narrow_source(a, (o + d.len() as u64).min(size).max(a)));
            }
            src
        }

        /// Applies `edits` as one patch, from [`Model::patch_source`], reading each shard no more
        /// often than the content holds it.
        fn patch(&mut self, edits: &[(u64, Vec<u8>)]) -> Edited {
            let src = self.patch_source(edits);
            let mut reads: HashMap<ShardHash, usize> = HashMap::new();
            let es: Vec<_> = edits.iter().map(|(o, d)| Edit { offset: *o, data: d }).collect();
            let e = apply_edits(&self.extents, &es, SMALL, &mut |h: &ShardHash| {
                *reads.entry(*h).or_default() += 1;
                src.get(h).cloned()
            })
            .unwrap();
            for (h, n) in reads {
                assert!(n <= self.extents.iter().filter(|e| e.shard() == Some(h)).count(), "shard {h} read {n} times");
            }
            let used: std::collections::HashSet<_> = e.extents.iter().filter_map(Extent::shard).collect();
            assert!(e.new_shards.iter().all(|s| used.contains(&s.hash)), "every new shard is used");
            assert_eq!(dedup(e.new_shards.clone()).len(), e.new_shards.len(), "new shards are distinct");
            self.absorb(e.clone());
            for (o, d) in edits {
                let (o, end) = (*o as usize, *o as usize + d.len());
                if self.bytes.len() < end {
                    self.bytes.resize(end, 0);
                }
                self.bytes[o..end].copy_from_slice(d);
            }
            e
        }
    }

    /// Patches as they were applied before edits were grouped: each edit a [`write_at`] on the
    /// result of the one before.
    fn edit_by_edit(extents: &[Extent], edits: &[Edit<'_>], p: Params, src: &mut impl ShardSource) -> Result<Edited, EditError> {
        let mut current = extents.to_vec();
        let mut made: HashMap<ShardHash, Bytes> = HashMap::new();
        for edit in edits {
            let mut both = |h: &ShardHash| made.get(h).cloned().or_else(|| src.shard(h));
            let next = write_at(&current, edit.offset, edit.data, p, &mut both)?;
            for s in next.new_shards {
                made.insert(s.hash, s.bytes);
            }
            current = next.extents;
        }
        let used: std::collections::HashSet<_> = current.iter().filter_map(Extent::shard).collect();
        let new_shards = made.into_iter().filter(|(h, _)| used.contains(h)).map(|(hash, bytes)| Shard { hash, bytes }).collect();
        Ok(Edited { extents: current, new_shards })
    }

    use crate::patch::Edit;

    #[derive(Debug, Clone)]
    enum Op {
        Write(u64, Vec<u8>),
        Splice(u64, u64, Vec<u8>),
        Resize(u64),
        Patch(Vec<(u64, Vec<u8>)>),
    }

    fn write() -> impl Strategy<Value = (u64, Vec<u8>)> {
        (0u64..12_000, prop::collection::vec(any::<u8>(), 0..3000))
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            write().prop_map(|(o, d)| Op::Write(o, d)),
            (0u64..12_000, 0u64..3000, prop::collection::vec(any::<u8>(), 0..3000)).prop_map(|(o, r, d)| Op::Splice(o, r, d)),
            (0u64..14_000).prop_map(Op::Resize),
            prop::collection::vec(write(), 1..8).prop_map(Op::Patch),
        ]
    }

    proptest! {
        #[test]
        fn edits_match_a_byte_buffer(seed in 0u64..1000, len in 0usize..10_000, ops in prop::collection::vec(op(), 1..12)) {
            Model::new(random_bytes(seed, len)).run(ops);
        }

        /// Content with data extents among its shards and zeros: an edit reads a data extent's
        /// bytes from the extent itself, and never adds one.
        #[test]
        fn edits_with_data_extents_match_a_byte_buffer(pieces in prop::collection::vec(piece(), 0..6), ops in prop::collection::vec(op(), 1..12)) {
            Model::of(pieces).run(ops);
        }

        /// A patch holds what its edits wrote, in order, over shards, data extents and zeros, and
        /// past the end.
        #[test]
        fn patches_match_a_byte_buffer(pieces in prop::collection::vec(piece(), 0..6), patches in prop::collection::vec(prop::collection::vec(write(), 1..16), 1..4)) {
            let mut m = Model::of(pieces);
            m.check();
            for edits in patches {
                m.patch(&edits);
                m.check();
            }
        }

        /// A patch of one edit is that write, extent for extent: grouping keeps what `splice` does
        /// at a region's edges. Most writes start near an extent's edge, where those rules apply.
        #[test]
        fn a_one_edit_patch_is_a_write(pieces in prop::collection::vec(piece(), 0..6), (o, d) in write(), near in any::<prop::sample::Index>(), by in 0u64..80, small in 1usize..80) {
            let mut m = Model::of(pieces);
            let (o, d) = if o % 4 == 0 { (o, d) } else {
                let edges: Vec<u64> = m.extents.iter().scan(0, |at, e| { *at += e.len(); Some(*at) }).chain([0]).collect();
                (edges[near.index(edges.len())] + by % 4 * 20, random_bytes(o, small))
            };
            let src = m.patch_source(&[(o, d.clone())]);
            let want = write_at(&m.extents, o, &d, SMALL, &mut |h: &ShardHash| src.get(h).cloned()).unwrap();
            prop_assert_eq!(m.patch(&[(o, d)]).extents, want.extents);
        }

        /// Edits whose neighbourhoods (the extents `needed_shards` names) do not overlap are
        /// chunked as they were edit by edit, in any order.
        #[test]
        fn patches_whose_edits_are_apart_match_edit_by_edit(pieces in prop::collection::vec(piece(), 1..8), edits in prop::collection::vec(write(), 1..16)) {
            let mut m = Model::of(pieces);
            let (size, n) = (m.bytes.len() as u64, m.extents.len());
            let mut kept = Vec::new();
            let mut taken: Vec<std::ops::Range<usize>> = Vec::new();
            for (o, d) in edits {
                let end = o + d.len() as u64;
                // A write reaching past the end adds to the end too.
                let w = neighbourhood(&m.extents, o.min(size), end.min(size));
                let w = if end > size { w.start.min(n.saturating_sub(1))..usize::MAX } else { w };
                if !d.is_empty() && !w.is_empty() && taken.iter().all(|t| t.end <= w.start || w.end <= t.start) {
                    taken.push(w);
                    kept.push((o, d));
                }
            }
            let src = m.patch_source(&kept);
            let es: Vec<_> = kept.iter().map(|(o, d)| Edit { offset: *o, data: d }).collect();
            let want = edit_by_edit(&m.extents, &es, SMALL, &mut |h: &ShardHash| src.get(h).cloned()).unwrap();
            prop_assert_eq!(m.patch(&kept).extents, want.extents);
            m.check();
        }
    }

    /// An edit that touches a data extent, or leaves a short region beside it, re-chunks its bytes
    /// into shards; one far from the edit is kept.
    #[test]
    fn edits_rechunk_the_data_extents_they_touch() {
        let data = |m: &Model| m.extents.iter().filter(|e| matches!(e, Extent::Data { .. })).count();
        let mut m = Model::of(vec![Piece::Data(b"hello world".to_vec())]);
        let e = write_at(&m.extents, 6, b"WORLD", SMALL, &mut |_: &ShardHash| None).unwrap();
        m.absorb(e);
        m.bytes[6..].copy_from_slice(b"WORLD");
        m.check();
        assert_eq!(data(&m), 0);
        // Appending to it absorbs it too, rather than leave a short shard beside it.
        let mut m = Model::of(vec![Piece::Data(b"hello".to_vec())]);
        let e = write_at(&m.extents, 5, b" world", SMALL, &mut |_: &ShardHash| None).unwrap();
        m.absorb(e);
        m.bytes.extend_from_slice(b" world");
        m.check();
        assert_eq!((data(&m), m.extents.len()), (0, 1));
        let mut m = Model::of(vec![Piece::Data(vec![7; 100]), Piece::Shards(random_bytes(1, 5000))]);
        let src = m.narrow_source(4000, 4004);
        let e = write_at(&m.extents, 4000, b"four", SMALL, &mut |h: &ShardHash| src.get(h).cloned()).unwrap();
        m.absorb(e);
        m.bytes[4000..4004].copy_from_slice(b"four");
        m.check();
        assert_eq!(data(&m), 1, "untouched");
    }

    #[test]
    fn small_content_is_one_data_extent_and_spills_to_one_shard() {
        assert!(inline(b"").is_empty());
        let big = vec![1u8; 1 << 20];
        let one = inline(&Bytes::from(big).slice(..MAX_DATA_BYTES));
        let [Extent::Data { d }] = &one[..] else { panic!("{one:?}") };
        assert_eq!(d.len(), MAX_DATA_BYTES);
        let spilled = spill(&one);
        assert_eq!(spilled.extents, [Extent::Shard { s: ShardHash::of(d), n: MAX_DATA_BYTES as u64 }]);
        assert_eq!(spilled.new_shards.len(), 1);
        assert_eq!(spill(&spilled.extents), Edited { extents: spilled.extents.clone(), new_shards: Vec::new() });
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
        let m = Model::of(vec![Piece::Shards(random_bytes(4, 5000)), Piece::Data(random_bytes(5, 3000)), Piece::Zeros(500), Piece::Data(vec![9; 10])]);
        for (start, len) in [(1000, 2500), (0, 8510), (4990, 3100), (8000, 510), (8505, 2)] {
            let mut got = Vec::new();
            for p in read_plan(&m.extents, start, len).unwrap() {
                match p.source {
                    Source::Shard(h) => got.extend_from_slice(&m.store[&h][p.offset as usize..(p.offset + p.len) as usize]),
                    Source::Zeros => got.resize(got.len() + p.len as usize, 0),
                    Source::Data(b) => {
                        assert_eq!(b.len() as u64, p.len);
                        got.extend_from_slice(&b);
                    }
                }
            }
            assert_eq!(got, &m.bytes[start as usize..(start + len) as usize], "{start}+{len}");
        }
        assert!(read_plan(&m.extents, 8000, 511).is_err());
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

    fn patch(edits: &[(u64, &[u8])]) -> Vec<(u64, Vec<u8>)> {
        edits.iter().map(|(o, d)| (*o, d.to_vec())).collect()
    }

    fn by_edit(m: &Model, edits: &[(u64, Vec<u8>)]) -> Vec<Extent> {
        let src = m.patch_source(edits);
        let es: Vec<_> = edits.iter().map(|(o, d)| Edit { offset: *o, data: d }).collect();
        edit_by_edit(&m.extents, &es, SMALL, &mut |h: &ShardHash| src.get(h).cloned()).unwrap().extents
    }

    #[test]
    fn later_edits_win_where_they_overlap() {
        let mut m = Model::new(random_bytes(1, 3000));
        m.patch(&patch(&[(100, b"aaaaaaaa"), (104, b"bbbbbbbb"), (106, b"c"), (2990, b"past the end"), (2995, b"d")]));
        m.check();
        assert_eq!(&m.bytes[100..112], b"aaaabbcbbbbb");
        assert_eq!(&m.bytes[2990..], b"past dhe end");
    }

    /// Every edit in one shard is applied to it before it is chunked, once, from its start: the
    /// result is what chunking the new bytes afresh gives. Edit by edit, each edit is chunked up
    /// to the boundaries the one before left.
    #[test]
    fn a_patch_within_one_shard_is_chunked_once() {
        let mut m = Model::new(random_bytes(185, 1000));
        assert_eq!(m.extents.len(), 1);
        let edits: Vec<_> = (0..16).map(|i| (i * 60, random_bytes(100 + i, 30))).collect();
        let before = by_edit(&m, &edits);
        let e = m.patch(&edits);
        m.check();
        assert_eq!(e.extents, from_bytes(&Bytes::from(m.bytes.clone()), SMALL).extents);
        assert_ne!(e.extents, before, "this patch is one where edit by edit leaves other boundaries");
    }

    /// Edits in neighbouring shards that do not cross the boundary between them stay apart, as
    /// they would edit by edit: each is chunked up to its own end, and the boundary stays.
    #[test]
    fn edits_in_neighbouring_shards_keep_the_boundary_between() {
        // Two runs of shards, so the boundary between them is not one chunking would find.
        let mut m = Model::of(vec![Piece::Shards(random_bytes(2, 3000)), Piece::Shards(random_bytes(3, 3000))]);
        let edits = patch(&[(2990, b"left"), (3004, b"right")]);
        let want = by_edit(&m, &edits);
        let e = m.patch(&edits);
        m.check();
        assert_eq!(e.extents, want);
        let mut at = 0;
        assert!(e.extents.iter().any(|x| {
            at += x.len();
            at == 3000
        }));
    }

    /// Writes into a zero run keep the zeros between them, and are joined only as an edit would
    /// join them: a short one with its neighbour.
    #[test]
    fn edits_in_a_zero_run_stay_apart() {
        let mut m = Model::of(vec![Piece::Zeros(10_000)]);
        let edits = patch(&[(1000, &[1; 100]), (5000, &[2; 100]), (1100, &[3; 100]), (8010, &[4; 10]), (8000, &[5; 10]), (9000, &[6; 100]), (9100, &[7; 10])]);
        let want = by_edit(&m, &edits);
        let e = m.patch(&edits);
        m.check();
        assert_eq!(e.extents, want);
        let kinds: Vec<_> = e.extents.iter().map(|x| (matches!(x, Extent::Zero { .. }), x.len())).collect();
        assert_eq!(kinds, [(true, 1000), (false, 100), (false, 100), (true, 3800), (false, 100), (true, 2900), (false, 20), (true, 980), (false, 110), (true, 890)]);
    }

    /// A short group pulls in the group after it, which may then pull in extents after itself,
    /// as its edit would have.
    #[test]
    fn a_short_group_pulls_the_next_and_what_follows_it() {
        let mut m = Model::of(vec![Piece::Zeros(120), Piece::Data(vec![1; 20]), Piece::Data(vec![2; 20]), Piece::Data(vec![3; 20]), Piece::Shards(random_bytes(6, 1000))]);
        let edits = patch(&[(100, &[4; 10]), (110, &[5; 10])]);
        let want = by_edit(&m, &edits);
        let e = m.patch(&edits);
        m.check();
        assert_eq!(e.extents, want);
        assert_eq!((e.extents[0].len(), e.extents[1].len()), (100, 60));
        assert!(matches!(e.extents[2], Extent::Data { .. }));
    }

    #[test]
    fn patches_past_the_end_extend_with_zeros() {
        let mut m = Model::new(random_bytes(4, 500));
        let edits = patch(&[(2000, b"far"), (498, b"across"), (5000, b"")]);
        let want = by_edit(&m, &edits);
        let e = m.patch(&edits);
        m.check();
        assert_eq!(size(&e.extents), 5000);
        assert_eq!(e.extents, want);
        assert_eq!(e.extents.last(), Some(&Extent::Zero { z: 2997 }));
    }

    /// A data extent an edit touches becomes shards; one no edit reaches is kept.
    #[test]
    fn patches_rechunk_the_data_extents_they_touch() {
        let data = |m: &Model| m.extents.iter().filter(|e| matches!(e, Extent::Data { .. })).count();
        let mut m = Model::of(vec![Piece::Data(vec![7; 100]), Piece::Shards(random_bytes(5, 5000)), Piece::Data(vec![8; 50])]);
        let edits = patch(&[(10, b"one"), (60, b"two"), (3000, b"three")]);
        let want = by_edit(&m, &edits);
        let e = m.patch(&edits);
        m.check();
        assert_eq!(e.extents, want);
        assert_eq!(data(&m), 1);
        assert_eq!(m.extents.last(), Some(&Extent::Data { d: Bytes::from(vec![8; 50]) }));
    }
}
