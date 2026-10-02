// SPDX-License-Identifier: Apache-2.0
//! Identifiers and timestamps used throughout the format (format §2, §7).

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, SubsecRound, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

/// SHA-256 of a shard's or page's bytes; its name in the bucket.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShardHash(pub [u8; 32]);

impl ShardHash {
    pub fn of(bytes: &[u8]) -> Self {
        ShardHash(Sha256::digest(bytes).into())
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// `<h0h1>/<h2h3>/<hex>`, the path under `shards/` or `pages/` (format §2).
    pub fn object_path(&self) -> String {
        let h = self.to_hex();
        format!("{}/{}/{}", &h[0..2], &h[2..4], h)
    }
}

impl fmt::Debug for ShardHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ShardHash({})", &self.to_hex()[..12])
    }
}

impl fmt::Display for ShardHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for ShardHash {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        if s.len() != 64 || s.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err(IdError::Hash(s.to_owned()));
        }
        let mut out = [0u8; 32];
        hex::decode_to_slice(s, &mut out).map_err(|_| IdError::Hash(s.to_owned()))?;
        Ok(ShardHash(out))
    }
}

/// A version id: transaction `idx` of commit `seq` (format §7.1). Clients treat it as opaque.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct VersionId {
    pub seq: u64,
    pub idx: u32,
}

impl VersionId {
    pub fn new(seq: u64, idx: u32) -> Self {
        VersionId { seq, idx }
    }
}

impl fmt::Display for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.seq, self.idx)
    }
}

impl FromStr for VersionId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        let bad = || IdError::Version(s.to_owned());
        let (seq, idx) = s.split_once('.').ok_or_else(bad)?;
        let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        if !digits(seq) || !digits(idx) {
            return Err(bad());
        }
        Ok(VersionId { seq: seq.parse().map_err(|_| bad())?, idx: idx.parse().map_err(|_| bad())? })
    }
}

/// An object id: `o-` and a ULID, or the reserved parent `root` (format §7.5, §7.6).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(Arc<str>);

impl ObjectId {
    pub const ROOT_STR: &'static str = "root";

    pub fn root() -> Self {
        ObjectId(Arc::from(Self::ROOT_STR))
    }

    /// Sorts before every id: a range's lower bound, never an object's id.
    pub(crate) fn lowest() -> Self {
        ObjectId(Arc::from(""))
    }

    pub fn generate() -> Self {
        ObjectId(Arc::from(format!("o-{}", ulid::Ulid::generate())))
    }

    pub fn is_root(&self) -> bool {
        &*self.0 == Self::ROOT_STR
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ObjectId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        if s == Self::ROOT_STR {
            return Ok(Self::root());
        }
        let ok = s.len() == 28
            && s.starts_with("o-")
            && s[2..].bytes().all(|b| b.is_ascii_digit() || (b.is_ascii_uppercase() && !b"ILOU".contains(&b)));
        if ok { Ok(ObjectId(Arc::from(s))) } else { Err(IdError::Object(s.to_owned())) }
    }
}

/// A drive id: `d-` and a lowercase UUID (format §2).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct DriveId(Arc<str>);

impl DriveId {
    pub fn generate() -> Self {
        DriveId(Arc::from(format!("d-{}", uuid::Uuid::now_v7())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DriveId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for DriveId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        let uuid = s.strip_prefix("d-").ok_or_else(|| IdError::Drive(s.to_owned()))?;
        match uuid::Uuid::parse_str(uuid) {
            Ok(u) if u.hyphenated().to_string() == uuid => Ok(DriveId(Arc::from(s))),
            _ => Err(IdError::Drive(s.to_owned())),
        }
    }
}

/// A UTC instant with microsecond precision, serialized as RFC 3339 with `Z`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    pub fn now() -> Self {
        Timestamp(Utc::now().trunc_subsecs(6))
    }

    pub fn from_datetime(dt: DateTime<Utc>) -> Self {
        Timestamp(dt.trunc_subsecs(6))
    }

    pub fn datetime(&self) -> DateTime<Utc> {
        self.0
    }

    pub fn max(self, other: Timestamp) -> Timestamp {
        if other > self { other } else { self }
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_rfc3339_opts(SecondsFormat::Micros, true))
    }
}

impl FromStr for Timestamp {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, IdError> {
        DateTime::parse_from_rfc3339(s)
            .map(|d| Timestamp::from_datetime(d.with_timezone(&Utc)))
            .map_err(|_| IdError::Time(s.to_owned()))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdError {
    #[error("invalid shard hash {0:?}")]
    Hash(String),
    #[error("invalid version id {0:?}")]
    Version(String),
    #[error("invalid object id {0:?}")]
    Object(String),
    #[error("invalid drive id {0:?}")]
    Drive(String),
    #[error("invalid timestamp {0:?}")]
    Time(String),
}

macro_rules! string_serde {
    ($($t:ty),*) => {$(
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    )*};
}
string_serde!(ShardHash, VersionId, ObjectId, DriveId, Timestamp);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ids_round_trip_and_order() {
        let v: VersionId = "42.3".parse().unwrap();
        assert_eq!(v, VersionId::new(42, 3));
        assert_eq!(v.to_string(), "42.3");
        assert!(VersionId::new(9, 5) < VersionId::new(10, 0));
        for bad in ["", "42", "42.", ".3", "4a.1", "-1.0", "1.0.0"] {
            assert!(bad.parse::<VersionId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn object_ids_are_validated() {
        let o = ObjectId::generate();
        assert_eq!(o.as_str().parse::<ObjectId>().unwrap(), o);
        assert!("root".parse::<ObjectId>().unwrap().is_root());
        assert!("o-short".parse::<ObjectId>().is_err());
        assert!("o-01J8ZILLEGALCHARACTERSXXXX".parse::<ObjectId>().is_err());
    }

    #[test]
    fn drive_ids_are_lowercase_uuids() {
        let d = DriveId::generate();
        assert_eq!(d.as_str().parse::<DriveId>().unwrap(), d);
        assert!("d-3F8E1B2A-4C5D-4E6F-8A9B-0C1D2E3F4A5B".parse::<DriveId>().is_err());
        assert!("x-3f8e1b2a-4c5d-4e6f-8a9b-0c1d2e3f4a5b".parse::<DriveId>().is_err());
    }

    #[test]
    fn timestamps_keep_microseconds() {
        let t: Timestamp = "2026-09-26T21:14:05.123456789Z".parse().unwrap();
        assert_eq!(t.to_string(), "2026-09-26T21:14:05.123456Z");
        let t2: Timestamp = "2026-09-26T23:14:05+02:00".parse().unwrap();
        assert_eq!(t2.to_string(), "2026-09-26T21:14:05.000000Z");
    }

    #[test]
    fn shard_paths_fan_out_by_prefix() {
        let h = ShardHash::of(b"hello");
        let hex = h.to_hex();
        assert_eq!(h.object_path(), format!("{}/{}/{}", &hex[..2], &hex[2..4], hex));
        assert_eq!(hex.parse::<ShardHash>().unwrap(), h);
        assert!(hex.to_uppercase().parse::<ShardHash>().is_err());
    }
}
