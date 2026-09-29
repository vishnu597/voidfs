// SPDX-License-Identifier: Apache-2.0
//! Content-defined chunking with FastCDC-2020, normalization level 1 (format §4.1).

use bytes::{Buf, Bytes, BytesMut};
use fastcdc::v2020::FastCDC;

use crate::ids::ShardHash;
use crate::model::Chunking;

/// Chunk-size bounds, validated against what FastCDC-2020 supports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Params {
    pub min: usize,
    pub avg: usize,
    pub max: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid chunking parameters min={min} avg={avg} max={max}")]
pub struct ParamsError {
    min: usize,
    avg: usize,
    max: usize,
}

impl Params {
    /// The pool default: 256 KiB / 2 MiB / 16 MiB.
    pub const DEFAULT: Params = Params { min: 262_144, avg: 2_097_152, max: 16_777_216 };

    pub fn new(min: usize, avg: usize, max: usize) -> Result<Params, ParamsError> {
        use fastcdc::v2020::*;
        let ok = (MINIMUM_MIN..=MINIMUM_MAX).contains(&min)
            && (AVERAGE_MIN..=AVERAGE_MAX).contains(&avg)
            && (MAXIMUM_MIN..=MAXIMUM_MAX).contains(&max)
            && min <= avg
            && avg <= max
            && [min, avg, max].iter().all(|v| v % 2 == 0);
        if ok { Ok(Params { min, avg, max }) } else { Err(ParamsError { min, avg, max }) }
    }

    pub fn from_pool(c: &Chunking) -> Result<Params, ParamsError> {
        Params::new(c.min as usize, c.avg as usize, c.max as usize)
    }
}

/// Cuts `data` into chunks; returns their lengths in order. Empty input gives no chunks.
pub fn cut(data: &[u8], p: Params) -> Vec<usize> {
    if data.is_empty() {
        return Vec::new();
    }
    FastCDC::new(data, p.min, p.avg, p.max).map(|c| c.length).collect()
}

/// A shard ready to store: its name and its bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Shard {
    pub hash: ShardHash,
    pub bytes: Bytes,
}

impl Shard {
    pub fn new(bytes: Bytes) -> Shard {
        Shard { hash: ShardHash::of(&bytes), bytes }
    }
}

/// Cuts `data` into shards.
pub fn shards(data: &Bytes, p: Params) -> Vec<Shard> {
    let mut at = 0;
    cut(data, p)
        .into_iter()
        .map(|len| {
            let s = Shard::new(data.slice(at..at + len));
            at += len;
            s
        })
        .collect()
}

/// Chunks a stream incrementally, holding at most one maximum-size chunk plus the latest
/// input. Produces the same shards as [`shards`] over the concatenated input.
pub struct StreamChunker {
    p: Params,
    buf: BytesMut,
}

impl StreamChunker {
    pub fn new(p: Params) -> Self {
        StreamChunker { p, buf: BytesMut::new() }
    }

    /// Adds input; returns every shard whose boundary is now certain.
    pub fn push(&mut self, data: &[u8]) -> Vec<Shard> {
        self.push_unhashed(data).into_iter().map(Shard::new).collect()
    }

    /// Ends the stream; returns the remaining shards.
    pub fn finish(self) -> Vec<Shard> {
        self.finish_unhashed().into_iter().map(Shard::new).collect()
    }

    /// [`StreamChunker::push`], leaving the shards' bytes for the caller to hash.
    pub fn push_unhashed(&mut self, data: &[u8]) -> Vec<Bytes> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        // A cut point depends on at most `max` bytes, so it is final once that much is held.
        while self.buf.len() >= self.p.max {
            let (_, end) = FastCDC::new(&self.buf, self.p.min, self.p.avg, self.p.max).cut(0, self.buf.len());
            // A copy, and the buffer advanced past it: splitting the shard off would share the
            // buffer, and the next input would then move all of it after the cut, up to `max`
            // bytes for every shard, to a new allocation.
            out.push(Bytes::copy_from_slice(&self.buf[..end]));
            self.buf.advance(end);
        }
        out
    }

    /// [`StreamChunker::finish`], leaving the shards' bytes for the caller to hash.
    pub fn finish_unhashed(mut self) -> Vec<Bytes> {
        let rest = self.buf.split().freeze();
        let mut at = 0;
        cut(&rest, self.p)
            .into_iter()
            .map(|len| {
                at += len;
                rest.slice(at - len..at)
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const SMALL: Params = Params { min: 64, avg: 256, max: 1024 };

    pub(crate) fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
        // xorshift: deterministic and dependency-free.
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    #[test]
    fn params_are_validated() {
        assert!(Params::new(262_144, 2_097_152, 16_777_216).is_ok());
        assert!(Params::new(64, 256, 1024).is_ok());
        assert!(Params::new(63, 256, 1024).is_err());
        assert!(Params::new(512, 256, 1024).is_err());
        assert!(Params::new(64, 256, 32 * 1024 * 1024).is_err());
        assert_eq!(Params::from_pool(&Chunking::default()).unwrap(), Params::DEFAULT);
    }

    #[test]
    fn chunks_respect_bounds_and_cover_input() {
        let data = random_bytes(1, 50_000);
        let lens = cut(&data, SMALL);
        assert_eq!(lens.iter().sum::<usize>(), data.len());
        assert!(lens[..lens.len() - 1].iter().all(|&l| (SMALL.min..=SMALL.max).contains(&l)));
        assert!(cut(&[], SMALL).is_empty());
    }

    #[test]
    fn streaming_matches_one_shot() {
        let data = Bytes::from(random_bytes(2, 40_000));
        let expected = shards(&data, SMALL);
        for piece in [1, 7, 333, 1024, 5000] {
            let mut c = StreamChunker::new(SMALL);
            let mut got = Vec::new();
            for part in data.chunks(piece) {
                got.extend(c.push(part));
            }
            got.extend(c.finish());
            assert_eq!(got, expected, "piece size {piece}");
        }
    }

    proptest::proptest! {
        /// However the input is split, the stream cuts what one pass over all of it does, and
        /// its buffer stays within a few times the largest shard and piece.
        #[test]
        fn streaming_matches_one_shot_for_any_split(seed in 0u64..1000, len in 0usize..20_000, pieces in proptest::collection::vec(1usize..3000, 1..20)) {
            let data = Bytes::from(random_bytes(seed, len));
            let largest = pieces.iter().max().copied().unwrap_or(1);
            let mut c = StreamChunker::new(SMALL);
            let mut got = Vec::new();
            let (mut at, mut i) = (0, 0);
            while at < data.len() {
                let n = pieces[i % pieces.len()].min(data.len() - at);
                got.extend(c.push(&data[at..at + n]));
                proptest::prop_assert!(c.buf.capacity() <= 4 * (SMALL.max + largest), "a buffer of {} bytes", c.buf.capacity());
                (at, i) = (at + n, i + 1);
            }
            got.extend(c.finish());
            proptest::prop_assert_eq!(got, shards(&data, SMALL));
        }
    }

    #[test]
    fn boundaries_reconverge_after_an_insert() {
        let data = random_bytes(3, 60_000);
        let mut edited = data.clone();
        edited.splice(30_000..30_000, b"inserted".iter().copied());
        let a: std::collections::HashSet<_> = shards(&Bytes::from(data), SMALL).into_iter().map(|s| s.hash).collect();
        let b = shards(&Bytes::from(edited), SMALL);
        let fresh = b.iter().filter(|s| !a.contains(&s.hash)).count();
        assert!(fresh <= 3, "{fresh} of {} shards changed", b.len());
    }
}
