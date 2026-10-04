// SPDX-License-Identifier: Apache-2.0
//! The case file, as serde types, plus request-body generation.

use std::collections::BTreeMap;

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize)]
pub struct Suite {
    pub format: u32,
    pub protocol: u32,
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub title: String,
    pub spec: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    #[serde(default = "admin")]
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
    /// A signed request; a step has this or `upload`.
    #[serde(default)]
    pub request: Option<Request>,
    /// PUTs to the URLs of a direct upload's plan (protocol §4.11), unsigned.
    #[serde(default)]
    pub upload: Option<Upload>,
    pub expect: Expect,
    /// Ends the case, skipped, when the response has this status.
    #[serde(default)]
    pub skip_if: Option<SkipIf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkipIf {
    pub status: u16,
    pub reason: String,
}

/// Content made from seeded bytes, with an optional edit: `text` written at `offset`. With
/// `unique`, the runner mixes a value of its own, fresh for each case, into the seed.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub seed: u64,
    pub size: u64,
    #[serde(default)]
    pub edit: Option<(u64, String)>,
    #[serde(default)]
    pub unique: bool,
}

impl Content {
    /// The bytes, for a case whose own value is `nonce`.
    pub fn bytes(&self, nonce: u64) -> Vec<u8> {
        let mut b = seeded_bytes(if self.unique { self.seed ^ nonce } else { self.seed }, self.size);
        if let Some((offset, text)) = &self.edit {
            let at = *offset as usize;
            b[at..at + text.len()].copy_from_slice(text.as_bytes());
        }
        b
    }

    /// Its shards, cut as format §4.1 says with the sizes of a default `voidfs.json`.
    pub fn shards(&self, nonce: u64) -> Vec<voidfs_core::chunk::Shard> {
        voidfs_core::chunk::shards(&bytes::Bytes::from(self.bytes(nonce)), voidfs_core::chunk::Params::DEFAULT)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitForm {
    pub seed: u64,
    pub size: u64,
    #[serde(default)]
    pub edit: Option<(u64, String)>,
    #[serde(default)]
    pub unique: bool,
    pub token: String,
}

impl CommitForm {
    pub fn content(&self) -> Content {
        Content { seed: self.seed, size: self.size, edit: self.edit.clone(), unique: self.unique }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    /// A plan's `upload` list, as captured from its response.
    pub list: String,
    pub content: Content,
    /// Positions in the list not to upload.
    #[serde(default)]
    pub skip: Vec<usize>,
    /// Send each shard with its last byte changed.
    #[serde(default)]
    pub corrupt: bool,
}

fn admin() -> String {
    "admin".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub unsigned_headers: Vec<String>,
    #[serde(default)]
    pub body: Option<Body>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Body {
    Text(String),
    Base64(String),
    Bytes(Content),
    Patch(Vec<(u64, String)>),
    Json(serde_json::Value),
    /// A direct upload's plan of the content (protocol §4.11).
    Plan(Content),
    /// A direct upload's commit of the content, with a token.
    Commit(CommitForm),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub status: Status,
    #[serde(default)]
    pub headers: BTreeMap<String, Matcher>,
    #[serde(default)]
    pub body: Option<BodyExpect>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Status {
    One(u16),
    Any(Vec<u16>),
}

impl Status {
    pub fn accepts(&self, s: u16) -> bool {
        match self {
            Status::One(x) => *x == s,
            Status::Any(xs) => xs.contains(&s),
        }
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::One(x) => write!(f, "{x}"),
            Status::Any(xs) => write!(f, "one of {xs:?}"),
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct Matcher {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub equals: Option<serde_json::Value>,
    #[serde(default)]
    pub matches: Option<String>,
    #[serde(default)]
    pub present: Option<bool>,
    #[serde(default)]
    pub contains: Option<serde_json::Value>,
    #[serde(default)]
    pub not_equals: Option<serde_json::Value>,
    #[serde(default)]
    pub count: Option<usize>,
    #[serde(default)]
    pub capture: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyExpect {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub base64: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub bytes: Option<Content>,
    #[serde(default)]
    pub s3_error: Option<String>,
    #[serde(default)]
    pub json: Vec<Matcher>,
    #[serde(default)]
    pub xml: Vec<Matcher>,
}

pub fn load(json: &str) -> anyhow::Result<Suite> {
    let suite: Suite = serde_json::from_str(json)?;
    anyhow::ensure!(suite.format == 1 && suite.protocol == 1, "unsupported suite format");
    Ok(suite)
}

/// `n` deterministic bytes from `seed` (README "Body forms").
pub fn seeded_bytes(seed: u64, n: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(n as usize);
    let mut i = 0u64;
    while (out.len() as u64) < n {
        let mut h = Sha256::new();
        h.update(b"voidfs-conformance");
        h.update(seed.to_be_bytes());
        h.update(i.to_be_bytes());
        out.extend_from_slice(&h.finalize());
        i += 1;
    }
    out.truncate(n as usize);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_suite_parses() {
        let s = load(crate::CASES_JSON).unwrap();
        assert!(s.cases.len() >= 30);
    }

    #[test]
    fn seeded_bytes_are_stable() {
        let a = seeded_bytes(1, 100);
        assert_eq!(a.len(), 100);
        assert_eq!(a, seeded_bytes(1, 100));
        assert_ne!(a, seeded_bytes(2, 100));
        assert_eq!(&seeded_bytes(1, 40)[..], &a[..40]);
    }
}
