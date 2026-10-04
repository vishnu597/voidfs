// SPDX-License-Identifier: Apache-2.0
//! Reading a drive's storage straight from the bucket with the credentials its server issues
//! (protocol §5.5), as a mount or a bulk reader does instead of reading through the server.

use bytes::Bytes;
use percent_encoding::utf8_percent_encode;

use crate::error::service_error;
use crate::retry::{self, Replay};
use crate::sign::{PATH, QUERY, Signer, query_string};
use crate::{Client, DEFAULT_REGION, Error, Result, StorageCredentials};

/// The storage one set of storage credentials reaches: GETs and listings of the paths they read,
/// path-style at their endpoint, under their root, signed with them. Requests are retried as the
/// SDK's reads are, but a `403` or a `400` (credentials refused or expired) is not.
#[derive(Clone)]
pub struct Storage {
    client: Client,
    signer: Signer,
    bucket: String,
    root: String,
    credentials: StorageCredentials,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage").field("bucket", &self.bucket).field("root", &self.root).field("credentials", &self.credentials).finish_non_exhaustive()
    }
}

impl Client {
    /// A reader of the storage that `credentials`, from [`Client::storage_credentials`], reach.
    pub fn storage(&self, credentials: StorageCredentials) -> Result<Storage> {
        let s = &credentials.storage;
        if s.backend != "s3" {
            return Err(Error::Invalid(format!("storage backend {:?} is not supported", s.backend)));
        }
        let endpoint = s.endpoint.as_deref().ok_or_else(|| Error::Invalid("the storage credentials name no endpoint".into()))?;
        let c = &s.credentials;
        let region = s.region.as_deref().filter(|r| !r.is_empty()).unwrap_or(DEFAULT_REGION);
        let signer = Signer::new(endpoint, &c.access_key_id, &c.secret_access_key)?.with_region(region).with_session_token(c.session_token.as_deref());
        Ok(Storage { client: self.clone(), signer, bucket: s.bucket.clone(), root: s.root.clone(), credentials })
    }
}

impl Storage {
    pub fn credentials(&self) -> &StorageCredentials {
        &self.credentials
    }

    /// The object at `path` under the pool's root (format §2), or `None` if there is none.
    pub async fn get(&self, path: &str) -> Result<Option<Bytes>> {
        let path = format!("/{}/{}", utf8_percent_encode(&self.bucket, QUERY), utf8_percent_encode(&format!("{}{path}", self.root), PATH));
        match self.send(&path, "").await {
            Ok(body) => Ok(Some(body)),
            Err(e) if e.status() == Some(404) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The names of the objects directly under `dir` (which ends in `/`) in the pool, sorted,
    /// after `after` if given: ListObjectsV2 delimited by `/`, every page.
    pub async fn list(&self, dir: &str, after: Option<&str>) -> Result<Vec<String>> {
        let prefix = format!("{}{dir}", self.root);
        let bucket = format!("/{}", utf8_percent_encode(&self.bucket, QUERY));
        let start = after.map(|a| format!("{prefix}{a}"));
        let mut names = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut pairs = vec![("delimiter", "/"), ("list-type", "2"), ("prefix", prefix.as_str())];
            match (&token, &start) {
                (Some(t), _) => pairs.push(("continuation-token", t.as_str())),
                (None, Some(s)) => pairs.push(("start-after", s.as_str())),
                (None, None) => {}
            }
            let body = self.send(&bucket, &query_string(pairs)).await?;
            let text = std::str::from_utf8(&body).map_err(|_| Error::Decode("a listing that is not UTF-8".into()))?;
            let doc = roxmltree::Document::parse(text).map_err(|e| Error::Decode(format!("a listing that is not XML: {e}")))?;
            let root = doc.root_element();
            let child = |n: roxmltree::Node, name: &str| n.children().find(|c| c.has_tag_name(name)).and_then(|c| c.text()).map(str::to_owned);
            for c in root.children().filter(|n| n.has_tag_name("Contents")) {
                let key = child(c, "Key").ok_or_else(|| Error::Decode("a listed object without a key".into()))?;
                let name = key.strip_prefix(&prefix).ok_or_else(|| Error::Decode(format!("listed {key:?} under {prefix:?}")))?;
                names.push(name.to_owned());
            }
            token = match child(root, "IsTruncated").as_deref() {
                Some("true") => Some(child(root, "NextContinuationToken").ok_or_else(|| Error::Decode("a truncated listing without a continuation token".into()))?),
                _ => break,
            };
        }
        Ok(names)
    }

    /// A GET of `path` (encoded) with `query`, retried as a read is.
    async fn send(&self, path: &str, query: &str) -> Result<Bytes> {
        let max = self.client.config().max_attempts.max(1);
        let mut attempt = 1;
        loop {
            let r = async {
                let signed = self.signer.sign("GET", path, query, Vec::new(), &[], Bytes::new())?;
                let request = reqwest::Request::try_from(signed).map_err(|e| Error::Invalid(e.to_string()))?;
                let resp = self.client.http().execute(request).await?;
                let status = resp.status().as_u16();
                let headers = resp.headers().clone();
                let body = resp.bytes().await?;
                if status >= 300 {
                    return Err(Error::Service(service_error(status, &headers, &body)));
                }
                Ok(body)
            }
            .await;
            match r {
                Err(e) if attempt < max && retry::may_retry(&e, Replay::Safe) => {
                    tokio::time::sleep(retry::delay(attempt, &e)).await;
                    attempt += 1;
                }
                r => return r,
            }
        }
    }
}
