// SPDX-License-Identifier: Apache-2.0
//! The voidfs storage engine: the on-bucket format (`spec/format.md`) as Rust types, and the
//! pure logic on top of it — names, content-defined chunking, in-place edits, patch bodies,
//! manifest trees, and the drive state machine that commits are applied to.
//!
//! Nothing here performs I/O. Callers fetch shards and pages and hand them in.

pub mod chunk;
pub mod content;
pub mod ids;
pub mod manifest;
pub mod model;
pub mod names;
pub mod ops;
pub mod patch;
pub mod state;

pub use ids::{DriveId, ObjectId, ShardHash, Timestamp, VersionId};

/// The format major version this crate reads and writes.
pub const FORMAT_VERSION: u32 = 1;
