// SPDX-License-Identifier: Apache-2.0
//! SpaceFS's 49 benchmark scenarios, with the figures they published.
//!
//! Source: <https://docs.spacefs.com/benchmarks/> and its methodology page, run
//! `20260920T055107Z`, build `s3sdk@fff9779`. Their harness is not published, so where a
//! scenario's name leaves something open, the choice made here is written next to it and in
//! `bench/README.md`.

use serde::{Deserialize, Serialize};

pub const KIB: u64 = 1024;
pub const MIB: u64 = 1024 * KIB;

/// The filter tabs on SpaceFS's chart: Edits 24, Reads 10, Writes 11, Metadata 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Reads,
    Writes,
    Edits,
    Metadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// 4 KiB written at offset 4 KiB.
    WriteAt,
    Append,
    InsertStart,
    InsertMiddle,
    DeleteStart,
    DeleteMiddle,
    TruncateEnd,
    /// `edits` × 4 KiB, spread evenly over the object, as one request.
    Patch { edits: usize },
}

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// GetObject of one object every worker reads; `stream` reads the body chunk by chunk and
    /// drops it, otherwise the body is collected into memory.
    Get { size: u64, stream: bool },
    /// A `len`-byte range at a random 4 KiB-aligned offset of one object.
    Range { size: u64, len: u64 },
    Head { size: u64 },
    /// ListObjectsV2 of a prefix holding `keys` objects.
    List { keys: usize },
    /// `count` objects, each read once per round.
    FanoutGet { count: usize, size: u64 },
    /// A new object per operation.
    Put { size: u64 },
    /// A PutObject over the worker's own existing object.
    Overwrite { size: u64 },
    /// `count` new objects per round.
    FanoutPut { count: usize, size: u64 },
    /// A new object per operation in `part`-byte parts, uploaded in parallel.
    Multipart { size: u64, part: u64 },
    /// An edit of the worker's own object. On a bare bucket: get it, change it, put it back.
    Edit { size: u64, edit: EditKind },
    /// Rename the worker's own object. On a bare bucket: copy, then delete.
    Rename { size: u64 },
    /// Move the worker's own folder of `files` objects. On a bare bucket: list, copy each, then
    /// delete them all.
    MoveDir { files: usize, size: u64 },
}

/// SpaceFS's published p50s, in milliseconds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Published {
    /// Through their layer (s3sdk).
    pub layer: f64,
    /// The bare bucket.
    pub bare: f64,
    /// Through their layer with its cache cleared, where they give it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_cold: Option<f64>,
}

impl Published {
    /// How many times faster the layer was than the bare bucket (below 1: slower).
    pub fn speedup(&self) -> f64 {
        self.bare / self.layer
    }
}

#[derive(Debug)]
pub struct Scenario {
    pub id: &'static str,
    /// SpaceFS's name for it.
    pub name: &'static str,
    pub family: Family,
    pub kind: Kind,
    /// Operations in flight at once.
    pub concurrency: usize,
    /// Measured operations per round (not SpaceFS's; they do not publish theirs).
    pub ops: usize,
    pub spacefs: Published,
}

impl Scenario {
    /// Bytes one operation moves, for reporting throughput.
    pub fn bytes_per_op(&self) -> u64 {
        match self.kind {
            Kind::Get { size, .. } | Kind::Put { size } | Kind::Overwrite { size } | Kind::Multipart { size, .. } => size,
            Kind::Range { len, .. } => len,
            Kind::FanoutGet { size, .. } | Kind::FanoutPut { size, .. } => size,
            Kind::Head { .. } | Kind::List { .. } | Kind::Edit { .. } | Kind::Rename { .. } | Kind::MoveDir { .. } => 0,
        }
    }
}

const fn p(layer: f64, bare: f64) -> Published {
    Published { layer, bare, layer_cold: None }
}

const fn cold(layer: f64, bare: f64, layer_cold: f64) -> Published {
    Published { layer, bare, layer_cold: Some(layer_cold) }
}

const fn s(id: &'static str, name: &'static str, family: Family, kind: Kind, concurrency: usize, ops: usize, spacefs: Published) -> Scenario {
    Scenario { id, name, family, kind, concurrency, ops, spacefs }
}

const fn edit(id: &'static str, name: &'static str, size: u64, edit: EditKind, spacefs: Published) -> Scenario {
    let ops = if size <= MIB { 64 } else { 32 };
    s(id, name, Family::Edits, Kind::Edit { size, edit }, 8, ops, spacefs)
}

use EditKind::*;
use Family::*;

/// All 49, in the order of SpaceFS's table (their layer's speed-up, largest first).
pub static ALL: &[Scenario] = &[
    s("range-64k-of-64m", "range 64 KiB of 64 MiB", Reads, Kind::Range { size: 64 * MIB, len: 64 * KIB }, 8, 200, p(1.3, 44.1)),
    s("get-4k", "get 4 KiB", Reads, Kind::Get { size: 4 * KIB, stream: false }, 8, 200, cold(1.1, 25.5, 27.5)),
    s("move-dir-200x64k", "move dir 200 × 64 KiB", Metadata, Kind::MoveDir { files: 200, size: 64 * KIB }, 8, 16, p(99.0, 1774.0)),
    s("get-64m", "get 64 MiB", Reads, Kind::Get { size: 64 * MIB, stream: false }, 8, 32, cold(47.2, 779.0, 299.0)),
    s("stream-get-256m", "stream get 256 MiB", Reads, Kind::Get { size: 256 * MIB, stream: true }, 8, 16, p(178.0, 2772.0)),
    s("stream-get-64m", "stream get 64 MiB", Reads, Kind::Get { size: 64 * MIB, stream: true }, 8, 32, p(45.1, 697.0)),
    edit("append-4k-64m", "append 4 KiB to 64 MiB", 64 * MIB, Append, p(111.0, 1668.0)),
    s("head", "head", Metadata, Kind::Head { size: 4 * KIB }, 8, 200, p(0.9, 12.1)),
    edit("truncate-4k-end-64m", "truncate 4 KiB, end of 64 MiB", 64 * MIB, TruncateEnd, p(131.0, 1677.0)),
    s("get-32m", "get 32 MiB", Reads, Kind::Get { size: 32 * MIB, stream: false }, 8, 32, cold(30.3, 356.0, 179.0)),
    edit("delete-4k-middle-64m", "delete 4 KiB, middle of 64 MiB", 64 * MIB, DeleteMiddle, p(152.0, 1718.0)),
    edit("insert-4k-middle-64m", "insert 4 KiB, middle of 64 MiB", 64 * MIB, InsertMiddle, p(152.0, 1667.0)),
    s("list-200", "list 200 keys", Metadata, Kind::List { keys: 200 }, 8, 200, p(3.0, 27.2)),
    edit("append-4k-32m", "append 4 KiB to 32 MiB", 32 * MIB, Append, p(109.0, 874.0)),
    s("rename-64m", "rename 64 MiB", Metadata, Kind::Rename { size: 64 * MIB }, 8, 32, p(108.0, 858.0)),
    edit("delete-4k-start-64m", "delete 4 KiB, start of 64 MiB", 64 * MIB, DeleteStart, p(244.0, 1685.0)),
    edit("insert-4k-start-64m", "insert 4 KiB, start of 64 MiB", 64 * MIB, InsertStart, p(248.0, 1690.0)),
    edit("write-at-4k-64m", "write at 4 KiB in 64 MiB", 64 * MIB, WriteAt, p(267.0, 1667.0)),
    s("fanout-get-200x256k-c32", "fanout get 200 × 256 KiB, 32 at once", Reads, Kind::FanoutGet { count: 200, size: 256 * KIB }, 32, 200, p(5.2, 32.1)),
    edit("truncate-4k-end-32m", "truncate 4 KiB, end of 32 MiB", 32 * MIB, TruncateEnd, p(146.0, 860.0)),
    s("fanout-get-1000x4k-c32", "fanout get 1000 × 4 KiB, 32 at once", Reads, Kind::FanoutGet { count: 1000, size: 4 * KIB }, 32, 1000, p(4.9, 26.1)),
    edit("write-at-4k-32m", "write at 4 KiB in 32 MiB", 32 * MIB, WriteAt, p(172.0, 873.0)),
    edit("delete-4k-middle-32m", "delete 4 KiB, middle of 32 MiB", 32 * MIB, DeleteMiddle, p(179.0, 861.0)),
    edit("delete-4k-start-32m", "delete 4 KiB, start of 32 MiB", 32 * MIB, DeleteStart, p(186.0, 859.0)),
    s("get-1m", "get 1 MiB", Reads, Kind::Get { size: MIB, stream: false }, 8, 200, cold(8.3, 37.5, 44.1)),
    edit("insert-4k-middle-32m", "insert 4 KiB, middle of 32 MiB", 32 * MIB, InsertMiddle, p(234.0, 883.0)),
    edit("insert-4k-start-32m", "insert 4 KiB, start of 32 MiB", 32 * MIB, InsertStart, p(251.0, 890.0)),
    s("fanout-get-1000x4k-c64", "fanout get 1000 × 4 KiB, 64 at once", Reads, Kind::FanoutGet { count: 1000, size: 4 * KIB }, 64, 1000, p(10.6, 28.0)),
    edit("patch-16x4k-64m", "patch 16 × 4 KiB in 64 MiB", 64 * MIB, Patch { edits: 16 }, p(785.0, 1662.0)),
    edit("patch-16x4k-32m", "patch 16 × 4 KiB in 32 MiB", 32 * MIB, Patch { edits: 16 }, p(627.0, 899.0)),
    edit("append-4k-1m", "append 4 KiB to 1 MiB", MIB, Append, p(106.0, 108.0)),
    s("put-64m", "put 64 MiB", Writes, Kind::Put { size: 64 * MIB }, 8, 32, p(1109.0, 978.0)),
    s("put-32m", "put 32 MiB", Writes, Kind::Put { size: 32 * MIB }, 8, 32, p(630.0, 523.0)),
    edit("delete-4k-middle-1m", "delete 4 KiB, middle of 1 MiB", MIB, DeleteMiddle, p(139.0, 102.0)),
    edit("insert-4k-middle-1m", "insert 4 KiB, middle of 1 MiB", MIB, InsertMiddle, p(136.0, 94.8)),
    edit("patch-16x4k-1m", "patch 16 × 4 KiB in 1 MiB", MIB, Patch { edits: 16 }, p(154.0, 106.0)),
    edit("delete-4k-start-1m", "delete 4 KiB, start of 1 MiB", MIB, DeleteStart, p(155.0, 106.0)),
    edit("write-at-4k-1m", "write at 4 KiB in 1 MiB", MIB, WriteAt, p(157.0, 101.0)),
    edit("insert-4k-start-1m", "insert 4 KiB, start of 1 MiB", MIB, InsertStart, p(178.0, 105.0)),
    s("put-1m", "put 1 MiB", Writes, Kind::Put { size: MIB }, 8, 200, p(126.0, 72.8)),
    s("multipart-256m-x16m", "multipart put 256 MiB × 16 MiB", Writes, Kind::Multipart { size: 256 * MIB, part: 16 * MIB }, 8, 16, p(3209.0, 1790.0)),
    s("overwrite-1m", "overwrite 1 MiB", Writes, Kind::Overwrite { size: MIB }, 8, 200, p(125.0, 65.3)),
    s("put-4k", "put 4 KiB", Writes, Kind::Put { size: 4 * KIB }, 8, 200, p(60.6, 30.1)),
    edit("truncate-4k-end-1m", "truncate 4 KiB, end of 1 MiB", MIB, TruncateEnd, p(174.0, 82.3)),
    s("fanout-put-1000x4k-c32", "fanout put 1000 × 4 KiB, 32 at once", Writes, Kind::FanoutPut { count: 1000, size: 4 * KIB }, 32, 1000, p(60.3, 27.1)),
    s("fanout-put-1000x4k-c64", "fanout put 1000 × 4 KiB, 64 at once", Writes, Kind::FanoutPut { count: 1000, size: 4 * KIB }, 64, 1000, p(67.1, 27.8)),
    s("multipart-64m-x8m", "multipart put 64 MiB × 8 MiB", Writes, Kind::Multipart { size: 64 * MIB, part: 8 * MIB }, 8, 16, p(1613.0, 660.0)),
    s("fanout-put-200x256k-c32", "fanout put 200 × 256 KiB, 32 at once", Writes, Kind::FanoutPut { count: 200, size: 256 * KIB }, 32, 200, p(134.0, 48.0)),
    s("overwrite-4k", "overwrite 4 KiB", Writes, Kind::Overwrite { size: 4 * KIB }, 8, 200, p(84.6, 27.7)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_spacefs_published_shape() {
        assert_eq!(ALL.len(), 49);
        let count = |f: Family| ALL.iter().filter(|s| s.family == f).count();
        assert_eq!((count(Edits), count(Reads), count(Writes), count(Metadata)), (24, 10, 11, 4));
        // SpaceFS: faster in 31 of 49.
        assert_eq!(ALL.iter().filter(|s| s.spacefs.speedup() >= 1.0).count(), 31);
        let mut ids: Vec<_> = ALL.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 49, "ids are unique");
        // Their table is sorted by speed-up, largest first.
        assert!(ALL.windows(2).all(|w| w[0].spacefs.speedup() >= w[1].spacefs.speedup() - 0.05));
    }
}
