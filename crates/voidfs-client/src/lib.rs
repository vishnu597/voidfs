// SPDX-License-Identifier: Apache-2.0
//! The voidfs client core (step 4, item 3 of the parity plan): what the daemon and the mount run.
//!
//! - [`store`]: the per-user state database.
//! - [`cache`] and [`fetch`]: the block cache, with read-ahead, and where it gets bytes.
//! - [`queue`] and [`journal`]: the write journal and the upload queue.

pub mod cache;
pub mod error;
pub mod fetch;
pub mod journal;
mod publish;
pub mod queue;
pub mod store;

pub use cache::{Cache, CacheConfig, Reader, Usage};
pub use error::{Error, Result};
pub use fetch::{ApiFetcher, Content, Fetch};
pub use journal::{Attrs, Base, BatchId, EntryId, Op, State};
pub use queue::{BatchStatus, Import, Item, Queue, QueueConfig, Scope, Status};
pub use store::{Store, default_dir};
