// SPDX-License-Identifier: Apache-2.0
//! `aws-chunked` request bodies (protocol §2), as the AWS CLI and SDKs send them, and the
//! upload checksums they carry.
//!
//! ```text
//! <hex size>[;chunk-signature=<sig>]\r\n<bytes>\r\n ... 0[;chunk-signature=<sig>]\r\n
//! [<trailer name>:<value>\r\n ...][x-amz-trailer-signature:<sig>\r\n]\r\n
//! ```

use base64::Engine;
use bytes::{Buf, Bytes, BytesMut};
use sha2::{Digest, Sha256};

use crate::sigv4::{hmac, sha256_hex};

const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const MAX_LINE: usize = 8192;

// Sixteen tables take the input 16 bytes at a time: about 5 GB/s on one core, where the default
// single table, a byte at a time, does 0.5 and was half of a large upload's CPU time.
type Tables = crc::Table<16>;
static CRC32: crc::Crc<u32, Tables> = crc::Crc::<u32, Tables>::new(&crc::CRC_32_ISO_HDLC);
static CRC32C: crc::Crc<u32, Tables> = crc::Crc::<u32, Tables>::new(&crc::CRC_32_ISCSI);
static CRC64NVME: crc::Crc<u64, Tables> = crc::Crc::<u64, Tables>::new(&crc::CRC_64_NVME);

/// A running upload checksum (`x-amz-checksum-*`).
pub enum Checksum {
    Crc32(crc::Digest<'static, u32, Tables>),
    Crc32c(crc::Digest<'static, u32, Tables>),
    Crc64Nvme(crc::Digest<'static, u64, Tables>),
    Sha1(sha1::Sha1),
    Sha256(Sha256),
}

impl Checksum {
    /// The checksum a header or trailer name asks for.
    pub fn for_header(name: &str) -> Option<Checksum> {
        Some(match name.to_ascii_lowercase().as_str() {
            "x-amz-checksum-crc32" => Checksum::Crc32(CRC32.digest()),
            "x-amz-checksum-crc32c" => Checksum::Crc32c(CRC32C.digest()),
            "x-amz-checksum-crc64nvme" => Checksum::Crc64Nvme(CRC64NVME.digest()),
            "x-amz-checksum-sha1" => Checksum::Sha1(sha1::Sha1::new()),
            "x-amz-checksum-sha256" => Checksum::Sha256(Sha256::new()),
            _ => return None,
        })
    }

    pub fn update(&mut self, data: &[u8]) {
        match self {
            Checksum::Crc32(d) | Checksum::Crc32c(d) => d.update(data),
            Checksum::Crc64Nvme(d) => d.update(data),
            Checksum::Sha1(h) => h.update(data),
            Checksum::Sha256(h) => h.update(data),
        }
    }

    /// The value as it appears in the header: base64 of the big-endian digest.
    pub fn finish(self) -> String {
        let bytes: Vec<u8> = match self {
            Checksum::Crc32(d) | Checksum::Crc32c(d) => d.finalize().to_be_bytes().to_vec(),
            Checksum::Crc64Nvme(d) => d.finalize().to_be_bytes().to_vec(),
            Checksum::Sha1(h) => h.finalize().to_vec(),
            Checksum::Sha256(h) => h.finalize().to_vec(),
        };
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChunkError {
    #[error("malformed aws-chunked body: {0}")]
    Malformed(String),
    #[error("a chunk signature does not match")]
    Signature,
    #[error("the body ended before the final chunk")]
    Incomplete,
}

/// What signed chunks are verified with.
pub struct Signing {
    pub key: [u8; 32],
    pub amz_date: String,
    pub scope: String,
    pub seed_signature: String,
}

enum State {
    Header,
    Data { left: u64, sig: Option<String>, hash: Option<Sha256> },
    DataEnd { sig: Option<String>, hash: Option<Sha256> },
    Trailers,
    Done,
}

pub struct Decoder {
    buf: BytesMut,
    state: State,
    signing: Option<Signing>,
    prev: String,
    trailers: Vec<(String, String)>,
    decoded: u64,
}

impl Decoder {
    pub fn new(signing: Option<Signing>) -> Decoder {
        let prev = signing.as_ref().map(|s| s.seed_signature.clone()).unwrap_or_default();
        Decoder { buf: BytesMut::new(), state: State::Header, signing, prev, trailers: Vec::new(), decoded: 0 }
    }

    fn line(&mut self) -> Result<Option<String>, ChunkError> {
        match self.buf.windows(2).position(|w| w == b"\r\n") {
            Some(i) => {
                let line = self.buf.split_to(i + 2);
                String::from_utf8(line[..i].to_vec()).map(Some).map_err(|_| ChunkError::Malformed("header is not UTF-8".into()))
            }
            None if self.buf.len() > MAX_LINE => Err(ChunkError::Malformed("header line too long".into())),
            None => Ok(None),
        }
    }

    fn verify_chunk(&mut self, sig: Option<String>, hash: Option<Sha256>) -> Result<(), ChunkError> {
        let Some(s) = &self.signing else { return Ok(()) };
        let sig = sig.ok_or_else(|| ChunkError::Malformed("chunk-signature missing".into()))?;
        let data_hash = hex::encode(hash.unwrap_or_default().finalize());
        let to_sign = format!("AWS4-HMAC-SHA256-PAYLOAD\n{}\n{}\n{}\n{EMPTY_SHA256}\n{data_hash}", s.amz_date, s.scope, self.prev);
        let want = hex::encode(hmac(&s.key, to_sign.as_bytes()));
        if want != sig {
            return Err(ChunkError::Signature);
        }
        self.prev = sig;
        Ok(())
    }

    /// Consumes input; returns the decoded content bytes available so far.
    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<Bytes>, ChunkError> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            match &mut self.state {
                State::Header => {
                    let Some(line) = self.line()? else { break };
                    let (size, ext) = line.split_once(';').unwrap_or((&line, ""));
                    let size = u64::from_str_radix(size.trim(), 16).map_err(|_| ChunkError::Malformed(format!("bad chunk size {size:?}")))?;
                    let sig = ext.trim().strip_prefix("chunk-signature=").map(str::to_owned);
                    let hash = self.signing.as_ref().map(|_| Sha256::new());
                    if size == 0 {
                        self.verify_chunk(sig, hash)?;
                        self.state = State::Trailers;
                    } else {
                        self.state = State::Data { left: size, sig, hash };
                    }
                }
                State::Data { left, sig, hash } => {
                    if self.buf.is_empty() {
                        break;
                    }
                    let n = (*left).min(self.buf.len() as u64) as usize;
                    let piece = self.buf.split_to(n).freeze();
                    if let Some(h) = hash {
                        h.update(&piece);
                    }
                    *left -= n as u64;
                    self.decoded += n as u64;
                    out.push(piece);
                    if *left == 0 {
                        self.state = State::DataEnd { sig: sig.take(), hash: hash.take() };
                    }
                }
                State::DataEnd { sig, hash } => {
                    if self.buf.len() < 2 {
                        break;
                    }
                    if &self.buf[..2] != b"\r\n" {
                        return Err(ChunkError::Malformed("chunk not followed by CRLF".into()));
                    }
                    self.buf.advance(2);
                    let (sig, hash) = (sig.take(), hash.take());
                    self.verify_chunk(sig, hash)?;
                    self.state = State::Header;
                }
                State::Trailers => {
                    let Some(line) = self.line()? else { break };
                    if line.is_empty() {
                        self.state = State::Done;
                        continue;
                    }
                    let (name, value) = line.split_once(':').ok_or_else(|| ChunkError::Malformed(format!("bad trailer {line:?}")))?;
                    self.trailers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
                }
                State::Done => {
                    if !self.buf.is_empty() {
                        return Err(ChunkError::Malformed("bytes after the final chunk".into()));
                    }
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Ends the body: checks it was complete, verifies the trailer signature, and returns the
    /// trailers (without the signature).
    pub fn finish(self) -> Result<(Vec<(String, String)>, u64), ChunkError> {
        if !matches!(self.state, State::Done) {
            return Err(ChunkError::Incomplete);
        }
        let (sigs, trailers): (Vec<_>, Vec<_>) = self.trailers.into_iter().partition(|(k, _)| k == "x-amz-trailer-signature");
        if let Some(s) = &self.signing {
            if let Some((_, sig)) = sigs.first() {
                let canonical: String = trailers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
                let to_sign = format!("AWS4-HMAC-SHA256-TRAILER\n{}\n{}\n{}\n{}", s.amz_date, s.scope, self.prev, sha256_hex(canonical.as_bytes()));
                if hex::encode(hmac(&s.key, to_sign.as_bytes())) != *sig {
                    return Err(ChunkError::Signature);
                }
            } else if !trailers.is_empty() {
                return Err(ChunkError::Malformed("signed trailers need x-amz-trailer-signature".into()));
            }
        }
        Ok((trailers, self.decoded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunked(data: &[u8], piece: usize, trailer: Option<(&str, String)>) -> Vec<u8> {
        let mut out = Vec::new();
        for c in data.chunks(piece) {
            out.extend_from_slice(format!("{:x}\r\n", c.len()).as_bytes());
            out.extend_from_slice(c);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\n");
        if let Some((k, v)) = trailer {
            out.extend_from_slice(format!("{k}:{v}\r\n").as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out
    }

    #[test]
    fn decodes_unsigned_chunks_with_a_checksum_trailer() {
        let data = b"hello chunked world, in several pieces".repeat(100);
        let mut c = Checksum::for_header("x-amz-checksum-crc32").unwrap();
        c.update(&data);
        let body = chunked(&data, 1000, Some(("x-amz-checksum-crc32", c.finish())));
        for split in [1, 7, 100, body.len()] {
            let mut d = Decoder::new(None);
            let mut got = Vec::new();
            for part in body.chunks(split) {
                for b in d.feed(part).unwrap() {
                    got.extend_from_slice(&b);
                }
            }
            let (trailers, n) = d.finish().unwrap();
            assert_eq!(got, data);
            assert_eq!(n, data.len() as u64);
            assert_eq!(trailers[0].0, "x-amz-checksum-crc32");
        }
    }

    #[test]
    fn known_checksum_values() {
        // "123456789" is the standard CRC check input.
        let check = |name: &str, want: &[u8]| {
            let mut c = Checksum::for_header(name).unwrap();
            c.update(b"123456789");
            assert_eq!(c.finish(), base64::engine::general_purpose::STANDARD.encode(want), "{name}");
        };
        check("x-amz-checksum-crc32", &0xcbf43926u32.to_be_bytes());
        check("x-amz-checksum-crc32c", &0xe3069283u32.to_be_bytes());
        check("x-amz-checksum-crc64nvme", &0xae8b14860a799888u64.to_be_bytes());
    }

    #[test]
    fn signed_chunks_are_verified() {
        let key = [7u8; 32];
        let signing = || Signing { key, amz_date: "20260926T000000Z".into(), scope: "20260926/us-east-1/s3/aws4_request".into(), seed_signature: "seed".into() };
        let sign = |prev: &str, data: &[u8]| {
            let to_sign = format!(
                "AWS4-HMAC-SHA256-PAYLOAD\n20260926T000000Z\n20260926/us-east-1/s3/aws4_request\n{prev}\n{EMPTY_SHA256}\n{}",
                sha256_hex(data)
            );
            hex::encode(hmac(&key, to_sign.as_bytes()))
        };
        let s1 = sign("seed", b"abc");
        let s2 = sign(&s1, b"");
        let body = format!("3;chunk-signature={s1}\r\nabc\r\n0;chunk-signature={s2}\r\n\r\n");
        let mut d = Decoder::new(Some(signing()));
        assert_eq!(d.feed(body.as_bytes()).unwrap().concat(), b"abc");
        assert!(d.finish().is_ok());
        let tampered = body.replace("abc", "abd");
        let mut d = Decoder::new(Some(signing()));
        assert_eq!(d.feed(tampered.as_bytes()).unwrap_err(), ChunkError::Signature);
    }

    #[test]
    fn truncated_and_malformed_bodies_fail() {
        let body = chunked(b"abc", 10, None);
        let mut d = Decoder::new(None);
        d.feed(&body[..body.len() - 3]).unwrap();
        assert_eq!(d.finish().unwrap_err(), ChunkError::Incomplete);
        let mut d = Decoder::new(None);
        assert!(matches!(d.feed(b"zz\r\n"), Err(ChunkError::Malformed(_))));
        let mut d = Decoder::new(None);
        assert!(matches!(d.feed(b"3\r\nabcXX"), Err(ChunkError::Malformed(_))));
    }
}
