// SPDX-License-Identifier: Apache-2.0
//! Watching a drive's change feed as Server-Sent Events (protocol §5.6).

use std::collections::VecDeque;
use std::time::Duration;

use http::Method;

use crate::client::{Patience, Req};
use crate::sign::drive_path;
use crate::types::ChangeBatch;
use crate::{Client, Error, Result, retry};

/// A drive's changes as they are published, from a position on. It keeps the position of the
/// last batch it delivered, and reconnects from there when the stream breaks.
///
/// An error doesn't end the watch: calling [`ChangeWatch::next`] again tries again from the same
/// position. `410 ChangesExpired` means the server no longer holds changes that old: relist
/// ([`Client::list_folder`]) and watch again from the listing's `seq`.
pub struct ChangeWatch {
    client: Client,
    drive: String,
    last: u64,
    stream: Option<reqwest::Response>,
    parser: EventParser,
    ready: VecDeque<ChangeBatch>,
    /// Streams in a row that broke before anything arrived.
    broken: u32,
}

impl std::fmt::Debug for ChangeWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangeWatch").field("drive", &self.drive).field("position", &self.last).field("connected", &self.stream.is_some()).finish_non_exhaustive()
    }
}

impl ChangeWatch {
    pub(crate) fn new(client: Client, drive: String, since: u64) -> ChangeWatch {
        ChangeWatch { client, drive, last: since, stream: None, parser: EventParser::default(), ready: VecDeque::new(), broken: 0 }
    }

    /// The position of the last batch delivered, or the one the watch started from.
    pub fn position(&self) -> u64 {
        self.last
    }

    /// Whether a stream is open now.
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// The next batch of changes, waiting for one.
    pub async fn next(&mut self) -> Result<ChangeBatch> {
        loop {
            if let Some(b) = self.ready.pop_front() {
                self.last = b.seq;
                return Ok(b);
            }
            let Some(stream) = self.stream.as_mut() else {
                if self.broken > 0 {
                    tokio::time::sleep(retry::delay(self.broken, &Error::Transport { message: String::new(), sent: true })).await;
                }
                // `since` says where to start; `Last-Event-ID` says the same, as a reconnecting
                // client of an event stream does (§5.6).
                let req = Req::new(Method::GET, drive_path(&self.drive))
                    .query("x-voidfs-changes", "")
                    .query("since", self.last.to_string())
                    .header("accept", "text/event-stream")
                    .header("last-event-id", self.last.to_string());
                let resp = self.client.execute_streaming(&req, Patience::Stream).await?;
                self.stream = Some(resp);
                self.parser = EventParser::default();
                continue;
            };
            // The server sends a comment every 15 s; silence much longer than that is a dead stream.
            let silence = self.client.config().timeout.max(Duration::from_secs(45));
            match tokio::time::timeout(silence, stream.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    self.broken = 0;
                    for data in self.parser.push(&chunk) {
                        let b: ChangeBatch = serde_json::from_str(&data).map_err(|e| Error::decode(format!("change event: {e}")))?;
                        if b.seq > self.last && self.ready.back().is_none_or(|r| b.seq > r.seq) {
                            self.ready.push_back(b);
                        }
                    }
                }
                Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {
                    self.stream = None;
                    self.broken += 1;
                    if self.broken > self.client.config().max_attempts.max(1) {
                        self.broken = 1;
                        return Err(Error::Transport { message: format!("the change feed of {} keeps breaking", self.drive), sent: true });
                    }
                }
            }
        }
    }
}

/// Server-Sent Events, fed in pieces: the data of each complete event.
#[derive(Default)]
struct EventParser {
    line: Vec<u8>,
    data: Option<String>,
}

impl EventParser {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in bytes {
            if b != b'\n' {
                self.line.push(b);
                continue;
            }
            if self.line.last() == Some(&b'\r') {
                self.line.pop();
            }
            let line = std::mem::take(&mut self.line);
            if line.is_empty() {
                if let Some(d) = self.data.take() {
                    out.push(d);
                }
                continue;
            }
            let (field, value) = match line.iter().position(|&c| c == b':') {
                Some(0) => continue, // a comment, such as a keep-alive
                Some(i) => (&line[..i], &line[i + 1..]),
                None => (&line[..], &[][..]),
            };
            let value = String::from_utf8_lossy(value.strip_prefix(b" ").unwrap_or(value));
            if field == b"data" {
                match &mut self.data {
                    Some(d) => {
                        d.push('\n');
                        d.push_str(&value);
                    }
                    None => self.data = Some(value.into_owned()),
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_split_across_pieces_and_comments_skipped() {
        let mut p = EventParser::default();
        assert!(p.push(b": keep-alive\n\nid: 4\nda").is_empty());
        assert_eq!(p.push(b"ta: {\"seq\":4}\r\n\r\nid: 5\ndata: a\ndata:b\n"), vec!["{\"seq\":4}".to_string()]);
        assert_eq!(p.push(b"\n"), vec!["a\nb".to_string()]);
        assert!(p.push(b"event: x\nid: 6\n\n").is_empty(), "an event without data is not dispatched");
    }
}
