// SPDX-License-Identifier: Apache-2.0
//! The voidfs client core (step 4, item 3 of the parity plan): what the daemon and the mount run.
//!
//! - [`store`]: the per-user state database.
//! - [`cache`] and [`fetch`]: the block cache, with read-ahead, and where it gets bytes.
//! - [`queue`] and [`journal`]: the write journal and the upload queue.
//! - [`feed`] and [`connectivity`]: the change-feed client, and whether the server can be reached.
//! - [`mounts`]: the mounts the daemon brings back when it starts.

pub mod cache;
pub mod connectivity;
pub mod error;
pub mod feed;
pub mod fetch;
pub mod journal;
pub mod mounts;
mod publish;
pub mod queue;
pub mod store;

pub use cache::{Cache, CacheConfig, Reader, Usage};
pub use connectivity::{Connectivity, ConnectivityConfig, Link};
pub use feed::{FeedEvent, FeedWatch, Invalidation, invalidations};
pub use error::{Error, Result};
pub use fetch::{ApiFetcher, Content, Fetch};
pub use journal::{Attrs, Base, BatchId, EntryId, Op, State};
pub use mounts::Remembered;
pub use queue::{BatchStatus, Import, Item, Queue, QueueConfig, Scope, Status};
pub use store::{Store, default_dir};
