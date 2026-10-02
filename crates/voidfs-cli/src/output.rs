// SPDX-License-Identifier: Apache-2.0
//! What commands print: text for people, or with `--json` one JSON document on stdout, and errors
//! on stderr in the same form, with a nonzero exit status.

use std::io::{self, Write};

use serde::Serialize;
use serde_json::{Value, json};

/// Why a command failed. Printed as `error: <message>`, or with `--json` as
/// `{"error": {"code", "status", "message", …}}` on stderr.
#[derive(Debug)]
pub struct Failure {
    /// The server's S3 code, for example `NoSuchKey`, or one of the CLI's own: `Usage`,
    /// `NoCredentials`, `InvalidArgument`, `RequestFailed`, `UnexpectedResponse`, `LocalFileError`,
    /// `NotConfirmed`.
    pub code: String,
    pub message: String,
    /// The HTTP status, for an error the server answered.
    pub status: Option<u16>,
    /// More members for the JSON form: `requestId`, and on a `412` `currentVersionId`, the
    /// object's current version.
    pub detail: serde_json::Map<String, Value>,
    /// 1, or 2 for a usage error.
    pub exit: u8,
    /// Nothing to print: stdout was closed under us, as by `| head`.
    pub quiet: bool,
}

pub type Result<T, E = Failure> = std::result::Result<T, E>;

impl Failure {
    pub fn new(code: &str, message: impl Into<String>) -> Failure {
        Failure { code: code.into(), message: message.into(), status: None, detail: Default::default(), exit: 1, quiet: false }
    }

    pub fn usage(message: impl Into<String>) -> Failure {
        Failure { exit: 2, ..Failure::new("Usage", message) }
    }

    pub fn invalid(message: impl Into<String>) -> Failure {
        Failure::new("InvalidArgument", message)
    }

    pub fn local(path: &std::path::Path, e: io::Error) -> Failure {
        Failure::new("LocalFileError", format!("{}: {e}", path.display()))
    }

    /// Says what the failure was about: `<what>: <message>`.
    pub fn context(mut self, what: impl std::fmt::Display) -> Failure {
        self.message = format!("{what}: {}", self.message);
        self
    }

    pub fn with(mut self, name: &str, value: impl Serialize) -> Failure {
        self.detail.insert(name.into(), serde_json::to_value(value).expect("JSON"));
        self
    }

    pub fn is(&self, code: &str) -> bool {
        self.code == code
    }

    pub fn to_json(&self) -> Value {
        let mut e = self.detail.clone();
        e.insert("code".into(), json!(self.code));
        if let Some(s) = self.status {
            e.insert("status".into(), json!(s));
        }
        e.insert("message".into(), json!(self.message));
        json!({ "error": e })
    }

    pub fn print(&self, json: bool) {
        if self.quiet {
            return;
        }
        let text = if json { pretty(&self.to_json()) } else { format!("error: {}", self.message) };
        let _ = writeln!(io::stderr().lock(), "{text}");
    }
}

impl From<voidfs_sdk::Error> for Failure {
    fn from(e: voidfs_sdk::Error) -> Failure {
        use voidfs_sdk::Error as E;
        match e {
            E::Service(s) => {
                let message = if s.message.is_empty() { format!("{} {}", s.status, s.code) } else { format!("{} {}: {}", s.status, s.code, s.message) };
                let mut f = Failure { status: Some(s.status), ..Failure::new(&s.code, message) };
                if let Some(r) = s.request_id {
                    f = f.with("requestId", r);
                }
                if let Some(v) = s.current_version_id {
                    f = f.with("currentVersionId", v);
                }
                f
            }
            E::Transport { message, sent } => Failure::new("RequestFailed", message).with("sent", sent),
            E::Decode(m) => Failure::new("UnexpectedResponse", m),
            E::Invalid(m) => Failure::invalid(m),
        }
    }
}

/// Where a command's output goes, and in which form.
#[derive(Clone, Copy, Debug)]
pub struct Out {
    pub json: bool,
}

impl Out {
    /// Prints `value` as JSON, or `text()` for people.
    pub fn emit(&self, value: &impl Serialize, text: impl FnOnce() -> String) -> Result<()> {
        if self.json { stdout(&pretty(&serde_json::to_value(value).expect("JSON"))) } else { stdout(&text()) }
    }
}

pub fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).expect("JSON")
}

/// Writes a line to stdout. A closed stdout ends the command quietly.
pub fn stdout(text: &str) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{text}").and_then(|_| out.flush()).map_err(closed)
}

/// The error for a write to stdout that failed.
pub fn closed(e: io::Error) -> Failure {
    if e.kind() == io::ErrorKind::BrokenPipe { Failure { quiet: true, exit: 0, ..Failure::new("BrokenPipe", "stdout closed") } } else { Failure::new("LocalFileError", format!("stdout: {e}")) }
}

/// A warning for people, on stderr.
pub fn warn(text: &str) {
    let _ = writeln!(io::stderr().lock(), "warning: {text}");
}

/// `s` as one word for a shell: as it is when that is safe, else in single quotes.
pub fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-:@%+=,".contains(&b)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `1.5 MiB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    format!("{v:.1} {}", UNITS[unit])
}

/// `n thing` or `n things`.
pub fn count(n: usize, thing: &str) -> String {
    if n == 1 { format!("1 {thing}") } else { format!("{n} {thing}s") }
}

/// Columns padded to their widest cell, two spaces apart.
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut width: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (w, c) in width.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let last = cells.len() - 1;
        let mut s = String::new();
        for (i, c) in cells.into_iter().enumerate() {
            s.push_str(c);
            if i < last {
                s.push_str(&" ".repeat(width[i] - c.chars().count() + 2));
            }
        }
        s
    };
    let mut out = vec![line(header.to_vec())];
    out.extend(rows.iter().map(|r| line(r.iter().map(String::as_str).collect())));
    out.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(5 << 30), "5.0 GiB");
        assert_eq!(count(1, "file"), "1 file");
        assert_eq!(count(2, "file"), "2 files");
    }

    #[test]
    fn shell_words_are_quoted_when_they_need_it() {
        assert_eq!(shell_word("cuts/a-1.txt"), "cuts/a-1.txt");
        assert_eq!(shell_word("my file.txt"), "'my file.txt'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn tables_line_up() {
        let t = table(&["NAME", "SIZE", ""], &[vec!["footage".into(), "1 B".into(), "".into()], vec!["é".into(), "10.0 KiB".into(), "current".into()]]);
        assert_eq!(t, "NAME     SIZE\nfootage  1 B\né        10.0 KiB  current");
    }

    #[test]
    fn errors_print_as_json_with_what_the_server_said() {
        let e = voidfs_sdk::Error::Service(voidfs_sdk::ServiceError {
            status: 412,
            code: "PreconditionFailed".into(),
            message: "no".into(),
            request_id: Some("r1".into()),
            current_version_id: Some("7.0".into()),
            retry_after: None,
        });
        let f = Failure::from(e).context("a.txt").with("key", "a.txt");
        assert_eq!(f.exit, 1);
        assert_eq!(
            f.to_json(),
            json!({ "error": { "code": "PreconditionFailed", "status": 412, "message": "a.txt: 412 PreconditionFailed: no", "requestId": "r1", "currentVersionId": "7.0", "key": "a.txt" } })
        );
        let f = Failure::from(voidfs_sdk::Error::Transport { message: "refused".into(), sent: false });
        assert_eq!(f.to_json(), json!({ "error": { "code": "RequestFailed", "message": "refused", "sent": false } }));
        assert_eq!(Failure::usage("x").exit, 2);
    }
}
