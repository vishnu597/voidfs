// SPDX-License-Identifier: Apache-2.0
//! Direct uploads (protocol §4.11): what a plan and its commit share. The URLs a store presigns
//! for the shards a plan lists, and the token that ties a commit to its plan.
//!
//! The token is stateless: the expiry and an HMAC over the drive, the key, the shard list and the
//! expiry, under a key this process makes when it starts. A commit recomputes the HMAC from its
//! own drive, key and list, so a token checks only for the plan it came from. A server that
//! restarts refuses its earlier tokens, and clients fall back to a put.

use std::time::Duration;

use base64::Engine;
use futures::future::BoxFuture;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use voidfs_core::ids::{DriveId, ShardHash};

/// How long a plan's token and upload URLs last (protocol §4.11).
pub const PLAN_TTL: Duration = Duration::from_secs(900);
/// The most shards one plan may list (protocol §7).
pub const MAX_SHARDS: usize = 4096;
/// The longest shard (format §4).
pub const MAX_SHARD: u64 = 16 << 20;

/// A request the store will accept without credentials: send it with exactly these headers.
#[derive(Clone, Debug)]
pub struct Presigned {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// A store that can presign a PUT. The headers given are signed, so the store refuses the PUT
/// without them, and enforces them if it implements them; the startup check
/// ([`crate::probe::presigned_puts`]) finds out which it does.
pub trait Presign: Send + Sync {
    /// A PUT of `path`, relative to the pool's root.
    fn put<'a>(&'a self, path: &'a str, headers: &'a [(String, String)], expires: Duration) -> BoxFuture<'a, anyhow::Result<Presigned>>;
}

/// Direct uploads as this server offers them: where the store binds a shard's checksum to its
/// URL, and maybe create-if-absent.
pub struct Direct {
    pub presign: Box<dyn Presign>,
    /// Whether URLs carry `If-None-Match: *`, which the store was found to enforce.
    pub if_none_match: bool,
    key: [u8; 32],
}

/// Why a commit's token does not hold.
#[derive(Debug, PartialEq, Eq)]
pub enum TokenError {
    Malformed,
    Expired,
    /// Not issued by this process for this drive, key and shard list.
    Mismatch,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::Malformed => write!(f, "the upload token is malformed"),
            TokenError::Expired => write!(f, "the upload token has expired; plan again"),
            TokenError::Mismatch => write!(f, "the upload token was not issued by this server for this drive, key and shard list"),
        }
    }
}

impl Direct {
    pub fn new(presign: Box<dyn Presign>, if_none_match: bool) -> Direct {
        Direct { presign, if_none_match, key: rand::random() }
    }

    /// A token for a plan of `shards` for `key` in `drive`, valid until `expires` (Unix seconds).
    pub fn token(&self, drive: &DriveId, key: &str, shards: &[(ShardHash, u64)], expires: i64) -> String {
        format!("{expires}.{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.mac(drive, key, shards, expires).finalize().into_bytes()))
    }

    /// Checks a commit's token against its own drive, key and shard list, at `now` (Unix seconds).
    pub fn check(&self, token: &str, drive: &DriveId, key: &str, shards: &[(ShardHash, u64)], now: i64) -> Result<(), TokenError> {
        let (expires, mac) = token.split_once('.').ok_or(TokenError::Malformed)?;
        let expires: i64 = expires.parse().map_err(|_| TokenError::Malformed)?;
        let mac = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(mac).map_err(|_| TokenError::Malformed)?;
        self.mac(drive, key, shards, expires).verify_slice(&mac).map_err(|_| TokenError::Mismatch)?;
        if now >= expires {
            return Err(TokenError::Expired);
        }
        Ok(())
    }

    fn mac(&self, drive: &DriveId, key: &str, shards: &[(ShardHash, u64)], expires: i64) -> Hmac<Sha256> {
        let mut m = Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts any key length");
        for field in [b"voidfs-upload-1".as_slice(), drive.as_str().as_bytes(), key.as_bytes(), &list_digest(shards), &expires.to_be_bytes()] {
            m.update(&(field.len() as u64).to_be_bytes());
            m.update(field);
        }
        m
    }
}

/// The SHA-256 of a shard list, in order: each hash and its length as a big-endian u64.
pub fn list_digest(shards: &[(ShardHash, u64)]) -> [u8; 32] {
    let mut h = Sha256::new();
    for (s, n) in shards {
        h.update(s.0);
        h.update(n.to_be_bytes());
    }
    h.finalize().into()
}

/// The `x-amz-checksum-sha256` value for a shard: its name is the SHA-256 of its bytes.
pub fn checksum(h: &ShardHash) -> String {
    base64::engine::general_purpose::STANDARD.encode(h.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoStore;

    impl Presign for NoStore {
        fn put<'a>(&'a self, _: &'a str, _: &'a [(String, String)], _: Duration) -> BoxFuture<'a, anyhow::Result<Presigned>> {
            Box::pin(async { anyhow::bail!("not a store") })
        }
    }

    #[test]
    fn a_token_holds_only_for_its_plan_and_until_it_expires() {
        let d = Direct::new(Box::new(NoStore), true);
        let drive: DriveId = "d-0192a7a4-8c1e-7b61-9d3f-3c6a1f0e2b77".parse().unwrap();
        let other: DriveId = "d-0192a7a4-8c1e-7b61-9d3f-3c6a1f0e2b78".parse().unwrap();
        let list = vec![(ShardHash::of(b"a"), 1), (ShardHash::of(b"bb"), 2)];
        let t = d.token(&drive, "k", &list, 1000);
        assert_eq!(d.check(&t, &drive, "k", &list, 999), Ok(()));
        assert_eq!(d.check(&t, &drive, "k", &list, 1000), Err(TokenError::Expired));
        assert_eq!(d.check(&t, &other, "k", &list, 999), Err(TokenError::Mismatch));
        assert_eq!(d.check(&t, &drive, "k2", &list, 999), Err(TokenError::Mismatch));
        let reversed: Vec<_> = list.iter().rev().cloned().collect();
        assert_eq!(d.check(&t, &drive, "k", &reversed, 999), Err(TokenError::Mismatch));
        assert_eq!(d.check(&t, &drive, "k", &[(ShardHash::of(b"a"), 2), (ShardHash::of(b"bb"), 2)], 999), Err(TokenError::Mismatch));
        // A later expiry written into the token does not verify.
        let (_, mac) = t.split_once('.').unwrap();
        assert_eq!(d.check(&format!("2000.{mac}"), &drive, "k", &list, 1500), Err(TokenError::Mismatch));
        assert_eq!(d.check("nonsense", &drive, "k", &list, 999), Err(TokenError::Malformed));
        // Another process's key does not verify.
        assert_eq!(Direct::new(Box::new(NoStore), true).check(&t, &drive, "k", &list, 999), Err(TokenError::Mismatch));
    }

    #[test]
    fn a_shards_checksum_is_its_name_in_base64() {
        assert_eq!(checksum(&ShardHash::of(b"")), "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=");
    }
}
