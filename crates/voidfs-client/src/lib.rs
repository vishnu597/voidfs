// SPDX-License-Identifier: Apache-2.0
//! The voidfs client core (step 4, item 3 of the parity plan): what the daemon and the mount run.
//!
//! - [`store`]: the per-user state database.
//! - [`cache`] and [`fetch`]: the block cache, with read-ahead, and where it gets bytes.

pub mod cache;
pub mod error;
pub mod fetch;
pub mod store;

pub use cache::{Cache, CacheConfig, Reader, Usage};
pub use error::{Error, Result};
pub use fetch::{ApiFetcher, Content, Fetch};
pub use store::{Store, default_dir};
