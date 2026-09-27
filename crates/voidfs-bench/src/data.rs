// SPDX-License-Identifier: Apache-2.0
//! Test data and the edits the edit scenarios make.
//!
//! Every object and every write gets fresh pseudo-random bytes. voidfs deduplicates content by
//! hash, so repeating a body, or changing only its first bytes, would let it skip uploads a real
//! workload pays for.

use crate::scenarios::EditKind;

/// The unit of every edit, as in SpaceFS's scenario names ("4 KiB").
pub const EDIT: u64 = 4096;

/// Mixes values into one seed (SplitMix64 finalizer over each part).
pub fn seed(parts: &[u64]) -> u64 {
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15;
    for &p in parts {
        h ^= p;
        h = h.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = h;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        h = z ^ (z >> 31);
    }
    h
}

/// `len` pseudo-random bytes from `seed` (wyrand: fast, and plenty for incompressible data).
pub fn random(seed: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0xa076_1d64_78bd_642f);
        let t = u128::from(state).wrapping_mul(u128::from(state ^ 0xe703_7ed1_a0b4_28db));
        ((t >> 64) ^ t) as u64
    };
    let (words, rest) = out.as_chunks_mut::<8>();
    for w in words {
        *w = next().to_le_bytes();
    }
    let last = next().to_le_bytes();
    rest.copy_from_slice(&last[..rest.len()]);
    out
}

/// Generates `len` random bytes off the async worker threads when the buffer is large, so that
/// generating one operation's body never delays another operation's I/O.
pub async fn random_async(seed: u64, len: usize) -> Vec<u8> {
    if len < (1 << 20) {
        return random(seed, len);
    }
    tokio::task::spawn_blocking(move || random(seed, len)).await.expect("generating data")
}

/// One concrete edit, planned against an object's current size.
#[derive(Clone, Debug)]
pub enum Change {
    /// pwrite: `data` at `offset`, extending the object if it runs past the end.
    Write { offset: u64, data: Vec<u8> },
    /// Remove `remove` bytes at `offset`, then insert `data` there.
    Splice { offset: u64, remove: u64, data: Vec<u8> },
    /// Cut the object to `size` bytes.
    Truncate { size: u64 },
    /// Several writes as one version.
    Patch { edits: Vec<(u64, Vec<u8>)> },
}

/// The middle of an object, on a 4 KiB boundary.
fn middle(size: u64) -> u64 {
    size / 2 / EDIT * EDIT
}

impl Change {
    /// The change `kind` makes to an object that is now `size` bytes, with fresh bytes.
    pub fn plan(kind: EditKind, size: u64, seed: u64) -> Change {
        let data = || random(seed, EDIT as usize);
        match kind {
            // "write at 4 KiB": 4 KiB written at offset 4 KiB, overwriting in place.
            EditKind::WriteAt => Change::Write { offset: EDIT, data: data() },
            EditKind::Append => Change::Write { offset: size, data: data() },
            EditKind::InsertStart => Change::Splice { offset: 0, remove: 0, data: data() },
            EditKind::InsertMiddle => Change::Splice { offset: middle(size), remove: 0, data: data() },
            EditKind::DeleteStart => Change::Splice { offset: 0, remove: EDIT, data: Vec::new() },
            EditKind::DeleteMiddle => Change::Splice { offset: middle(size), remove: EDIT, data: Vec::new() },
            EditKind::TruncateEnd => Change::Truncate { size: size - EDIT },
            // Spread evenly over the object, one edit per sixteenth.
            EditKind::Patch { edits } => {
                let stride = size / edits as u64 / EDIT * EDIT;
                let all = random(seed, edits * EDIT as usize);
                Change::Patch {
                    edits: all.chunks(EDIT as usize).enumerate().map(|(i, c)| (i as u64 * stride, c.to_vec())).collect(),
                }
            }
        }
    }

    pub fn size_after(&self, size: u64) -> u64 {
        match self {
            Change::Write { offset, data } => size.max(offset + data.len() as u64),
            Change::Splice { remove, data, .. } => size - remove + data.len() as u64,
            Change::Truncate { size } => *size,
            Change::Patch { edits } => edits.iter().map(|(o, d)| o + d.len() as u64).fold(size, u64::max),
        }
    }

    /// Applies the change to a whole object held in memory: what a client of a bare bucket has
    /// to do between downloading the object and uploading it again.
    pub fn apply(&self, buf: &mut Vec<u8>) {
        let write = |buf: &mut Vec<u8>, offset: u64, data: &[u8]| {
            let (o, end) = (offset as usize, offset as usize + data.len());
            if buf.len() < end {
                buf.resize(end, 0);
            }
            buf[o..end].copy_from_slice(data);
        };
        match self {
            Change::Write { offset, data } => write(buf, *offset, data),
            Change::Splice { offset, remove, data } => {
                let o = *offset as usize;
                buf.splice(o..o + *remove as usize, data.iter().copied());
            }
            Change::Truncate { size } => buf.truncate(*size as usize),
            Change::Patch { edits } => {
                for (o, d) in edits {
                    write(buf, *o, d);
                }
            }
        }
    }

    /// The request body of `?x-voidfs-patch` (protocol §4.2).
    pub fn patch_body(&self) -> Option<Vec<u8>> {
        match self {
            Change::Patch { edits } => {
                let e: Vec<_> = edits.iter().map(|(o, d)| voidfs_core::patch::Edit { offset: *o, data: d }).collect();
                Some(voidfs_core::patch::encode(&e))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_is_deterministic_and_distinct() {
        assert_eq!(random(7, 100), random(7, 100));
        assert_ne!(random(7, 100), random(8, 100));
        assert_eq!(random(1, 13).len(), 13);
        assert_ne!(seed(&[1, 2]), seed(&[2, 1]));
    }

    #[test]
    fn edits_match_their_sizes() {
        let size = 1 << 20;
        let base = random(1, size as usize);
        for kind in [
            EditKind::WriteAt,
            EditKind::Append,
            EditKind::InsertStart,
            EditKind::InsertMiddle,
            EditKind::DeleteStart,
            EditKind::DeleteMiddle,
            EditKind::TruncateEnd,
            EditKind::Patch { edits: 16 },
        ] {
            let c = Change::plan(kind, size, 2);
            let mut buf = base.clone();
            c.apply(&mut buf);
            assert_eq!(buf.len() as u64, c.size_after(size), "{kind:?}");
        }
        let mut buf = base.clone();
        Change::plan(EditKind::DeleteMiddle, size, 3).apply(&mut buf);
        assert_eq!(buf[..(size / 2) as usize], base[..(size / 2) as usize]);
        assert_eq!(buf[(size / 2) as usize..], base[(size / 2 + EDIT) as usize..]);
    }

    #[test]
    fn patch_body_round_trips() {
        let c = Change::plan(EditKind::Patch { edits: 16 }, 32 << 20, 4);
        let body = c.patch_body().unwrap();
        let edits = voidfs_core::patch::decode(&body).unwrap();
        assert_eq!(edits.len(), 16);
        assert_eq!(edits[1].offset, 2 << 20);
    }
}
