// SPDX-License-Identifier: Apache-2.0
//! Where the cache gets the bytes it doesn't have.
//!
//! [`ApiFetcher`] reads through the protocol, a ranged GET of one version. A fetcher that reads
//! shards straight from the bucket with storage credentials (step 4, item 6) implements the same
//! trait; it needs the protocol additions item 6 is waiting on (an object's shard list, or
//! presigned shard URLs), so it isn't here.

use bytes::Bytes;
use futures::future::BoxFuture;
use voidfs_sdk::{ObjectMeta, ReadOptions};

use crate::connectivity::Connectivity;
use crate::error::{Error, Result};

/// One version of a file: what the cache reads and keys its blocks by. An ETag names one
/// content, so blocks of it never go stale.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Content {
    pub drive: String,
    pub key: String,
    /// The version to read. Empty reads whatever is current, checked against `etag`.
    pub version_id: String,
    pub etag: String,
    pub size: u64,
}

impl Content {
    /// The version a HEAD or GET described.
    pub fn of(drive: &str, key: &str, meta: &ObjectMeta) -> Content {
        Content { drive: drive.to_owned(), key: key.to_owned(), version_id: meta.version_id.clone(), etag: meta.etag.clone(), size: meta.size }
    }
}

/// Reads `len` bytes at `offset` of a content version. The cache asks for whole blocks, and checks
/// the length it gets; a fetcher checks that the bytes are of that version.
pub trait Fetch: Send + Sync + 'static {
    fn fetch<'a>(&'a self, content: &'a Content, offset: u64, len: u64) -> BoxFuture<'a, Result<Bytes>>;
}

/// Reads through the API: `GET ?versionId=` with a range, which the SDK retries as it may.
pub struct ApiFetcher {
    client: voidfs_sdk::Client,
    conn: Option<Connectivity>,
}

impl ApiFetcher {
    pub fn new(client: voidfs_sdk::Client) -> ApiFetcher {
        ApiFetcher { client, conn: None }
    }

    /// Fails at once with [`Error::Offline`] while `conn` says the server can't be reached.
    pub fn with_connectivity(mut self, conn: Connectivity) -> ApiFetcher {
        self.conn = Some(conn);
        self
    }
}

impl Fetch for ApiFetcher {
    fn fetch<'a>(&'a self, c: &'a Content, offset: u64, len: u64) -> BoxFuture<'a, Result<Bytes>> {
        Box::pin(async move {
            if let Some(conn) = &self.conn {
                conn.check()?;
            }
            let opts = ReadOptions { version_id: (!c.version_id.is_empty()).then(|| c.version_id.clone()), ..Default::default() };
            let r = self.client.get_range(&c.drive, &c.key, offset, Some(len), opts).await?;
            let changed = |got: &str| Error::Changed { key: c.key.clone(), expected: c.etag.clone(), got: got.to_owned() };
            if !c.etag.is_empty() && r.meta.etag != c.etag {
                return Err(changed(&r.meta.etag));
            }
            if !c.version_id.is_empty() && r.meta.version_id != c.version_id {
                return Err(changed(&r.meta.version_id));
            }
            if r.body.len() as u64 != len {
                return Err(Error::ShortRead { key: c.key.clone(), offset, expected: len, got: r.body.len() as u64 });
            }
            Ok(r.body)
        })
    }
}
