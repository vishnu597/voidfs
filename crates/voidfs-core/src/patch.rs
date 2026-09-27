// SPDX-License-Identifier: Apache-2.0
//! The `application/vnd.voidfs.patch` body (protocol §4.2).

pub const MAGIC: &[u8; 4] = b"VFSP";
pub const VERSION: u8 = 1;
pub const MAX_EDITS: u32 = 10_000;
/// Largest extension request body (protocol §7).
pub const MAX_BODY: usize = 64 * 1024 * 1024;

const HEADER: usize = 12;
const EDIT_HEADER: usize = 16;

/// One edit: write `data` at `offset`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Edit<'a> {
    pub offset: u64,
    pub data: &'a [u8],
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatchError {
    #[error("patch body is larger than 64 MiB")]
    TooLarge,
    #[error("patch body is too short for its header")]
    Truncated,
    #[error("patch body does not start with VFSP")]
    BadMagic,
    #[error("patch version {0} is not supported")]
    BadVersion(u8),
    #[error("patch reserved bytes are not zero")]
    BadReserved,
    #[error("patch edit count {0} is outside 1 to 10,000")]
    BadCount(u32),
    #[error("patch edit {0} runs past the end of the body")]
    EditTruncated(u32),
    #[error("patch edit {0} overflows the offset range")]
    Overflow(u32),
    #[error("patch body has {0} trailing bytes")]
    Trailing(usize),
}

/// Parses and validates a whole patch body.
pub fn decode(body: &[u8]) -> Result<Vec<Edit<'_>>, PatchError> {
    if body.len() > MAX_BODY {
        return Err(PatchError::TooLarge);
    }
    if body.len() < HEADER {
        return Err(PatchError::Truncated);
    }
    if &body[..4] != MAGIC {
        return Err(PatchError::BadMagic);
    }
    if body[4] != VERSION {
        return Err(PatchError::BadVersion(body[4]));
    }
    if body[5..8] != [0, 0, 0] {
        return Err(PatchError::BadReserved);
    }
    let count = u32::from_be_bytes(body[8..12].try_into().unwrap());
    if !(1..=MAX_EDITS).contains(&count) {
        return Err(PatchError::BadCount(count));
    }
    let mut edits = Vec::with_capacity(count as usize);
    let mut at = HEADER;
    for i in 0..count {
        if body.len() - at < EDIT_HEADER {
            return Err(PatchError::EditTruncated(i));
        }
        let offset = u64::from_be_bytes(body[at..at + 8].try_into().unwrap());
        let length = u64::from_be_bytes(body[at + 8..at + 16].try_into().unwrap());
        at += EDIT_HEADER;
        let length = usize::try_from(length).map_err(|_| PatchError::EditTruncated(i))?;
        if body.len() - at < length {
            return Err(PatchError::EditTruncated(i));
        }
        offset.checked_add(length as u64).ok_or(PatchError::Overflow(i))?;
        edits.push(Edit { offset, data: &body[at..at + length] });
        at += length;
    }
    if at != body.len() {
        return Err(PatchError::Trailing(body.len() - at));
    }
    Ok(edits)
}

/// Encodes edits as a patch body.
pub fn encode(edits: &[Edit<'_>]) -> Vec<u8> {
    let len = HEADER + edits.iter().map(|e| EDIT_HEADER + e.data.len()).sum::<usize>();
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&[VERSION, 0, 0, 0]);
    out.extend_from_slice(&(edits.len() as u32).to_be_bytes());
    for e in edits {
        out.extend_from_slice(&e.offset.to_be_bytes());
        out.extend_from_slice(&(e.data.len() as u64).to_be_bytes());
        out.extend_from_slice(e.data);
    }
    out
}

/// The size a patched object has when no explicit size is given (protocol §4.2).
pub fn default_size(current: u64, edits: &[Edit<'_>]) -> u64 {
    edits.iter().map(|e| e.offset + e.data.len() as u64).fold(current, u64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let edits = [Edit { offset: 0, data: b"H" }, Edit { offset: 20, data: b"!" }, Edit { offset: 7, data: b"" }];
        let body = encode(&edits);
        assert_eq!(decode(&body).unwrap(), edits);
        assert_eq!(default_size(11, &edits), 21);
    }

    #[test]
    fn rejects_malformed_bodies() {
        let good = encode(&[Edit { offset: 0, data: b"ab" }]);
        assert_eq!(decode(b"not a patch"), Err(PatchError::Truncated));
        assert_eq!(decode(b"XXXX\x01\0\0\0\0\0\0\x01"), Err(PatchError::BadMagic));
        let mut v2 = good.clone();
        v2[4] = 2;
        assert_eq!(decode(&v2), Err(PatchError::BadVersion(2)));
        let mut reserved = good.clone();
        reserved[6] = 1;
        assert_eq!(decode(&reserved), Err(PatchError::BadReserved));
        assert_eq!(decode(b"VFSP\x01\0\0\0\0\0\0\0"), Err(PatchError::BadCount(0)));
        assert_eq!(decode(&good[..good.len() - 1]), Err(PatchError::EditTruncated(0)));
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(decode(&trailing), Err(PatchError::Trailing(1)));
        // The conformance suite's fixture: one edit claiming 10 bytes but carrying 2.
        let fixture = [&b"VFSP\x01\0\0\0\0\0\0\x01"[..], &0u64.to_be_bytes(), &10u64.to_be_bytes(), b"ab"].concat();
        assert_eq!(decode(&fixture), Err(PatchError::EditTruncated(0)));
        let overflow = [&b"VFSP\x01\0\0\0\0\0\0\x01"[..], &u64::MAX.to_be_bytes(), &1u64.to_be_bytes(), b"a"].concat();
        assert_eq!(decode(&overflow), Err(PatchError::Overflow(0)));
    }
}
