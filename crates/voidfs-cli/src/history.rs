// SPDX-License-Identifier: Apache-2.0
//! `history`, `show` and `restore` (protocol §4.4–§4.6, §4.10).

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use futures::{StreamExt, TryStreamExt};
use serde::Serialize;
use serde_json::json;
use voidfs_sdk::{Client, Kind, ListOptions, ReadOptions, VersionEntry};

use crate::output::{Failure, Out, Result, closed, shell_word, size, table};

/// A file or folder in a drive: `<drive> <path>`, or `--bucket <drive> <path>` as SpaceFS's CLI
/// takes it.
#[derive(clap::Args, Debug, Clone)]
pub struct Target {
    /// The drive, by alias or id. Leave it out when --bucket names it
    #[arg(value_name = "DRIVE")]
    first: String,
    /// The path within the drive, for example `cuts/a.mov`. A folder's may end in `/`
    #[arg(value_name = "PATH")]
    second: Option<String>,
    /// The drive, as SpaceFS's CLI names it: `--bucket <drive> <path>`
    #[arg(long, value_name = "DRIVE")]
    bucket: Option<String>,
}

impl Target {
    /// The drive, and the key: the path without a leading `/`.
    pub fn resolve(&self) -> Result<(String, String)> {
        let (drive, path) = match (&self.bucket, &self.second) {
            (Some(b), None) => (b.clone(), self.first.clone()),
            (None, Some(p)) => (self.first.clone(), p.clone()),
            (Some(_), Some(_)) => return Err(Failure::usage("give the drive once: `<drive> <path>` or `--bucket <drive> <path>`")),
            (None, None) => return Err(Failure::usage("give the drive and the path within it: `<drive> <path>`")),
        };
        let key = path.trim_start_matches('/').to_owned();
        if key.is_empty() {
            return Err(Failure::usage("give a path within the drive, for example `cuts/a.mov`"));
        }
        Ok((drive, key))
    }
}

/// `--at`: RFC 3339, or Unix seconds, as an RFC 3339 instant in UTC with microseconds.
pub fn parse_at(s: &str) -> Result<String> {
    let t = if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse().ok().and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
    } else {
        DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
    };
    t.map(|t| t.to_rfc3339_opts(SecondsFormat::Micros, true))
        .ok_or_else(|| Failure::invalid(format!("--at {s:?}: give a time as RFC 3339, for example 2026-06-15T12:00:00Z, or as Unix seconds")))
}

fn no_such_key(e: &voidfs_sdk::Error) -> bool {
    e.code() == Some("NoSuchKey")
}

/// The failure for a `404` on `key`, saying so if the object was deleted and how to bring it back.
async fn missing(client: &Client, drive: &str, key: &str, f: Failure) -> Failure {
    let base = key.trim_end_matches('/');
    let found = client.list_deleted(drive, base).await.ok().and_then(|d| d.into_iter().rev().find(|d| d.key == base || d.key == format!("{base}/")));
    match found {
        Some(d) => {
            let message = format!("{} was deleted at {}; `void restore {} {} --version {}` brings it back", d.key, d.deleted_at, shell_word(drive), shell_word(&d.key), d.last_version_id);
            Failure { message, ..f }.with("deleted", &d)
        }
        None => f.context(key),
    }
}

/// Says that a `404 NoSuchVersion` for a time means the object didn't exist then.
fn at_context(e: voidfs_sdk::Error, key: &str, at: Option<&str>) -> Failure {
    match at {
        Some(t) if e.code() == Some("NoSuchVersion") => Failure { message: format!("{key} did not exist at {t}"), ..Failure::from(e) },
        _ => Failure::from(e).context(key),
    }
}

async fn is_folder(client: &Client, drive: &str, folder: &str) -> Result<bool> {
    match client.head_object(drive, folder, ReadOptions::default()).await {
        Ok(m) => Ok(m.kind == Kind::Folder),
        Err(e) if e.is_not_found() => Ok(false),
        Err(e) => Err(Failure::from(e).context(folder)),
    }
}

/// Histories read at once for a folder.
const HISTORIES_AT_ONCE: usize = 8;

/// A version of a file in a folder.
#[derive(Serialize)]
struct InFolder {
    key: String,
    #[serde(flatten)]
    version: VersionEntry,
}

fn operation(v: &VersionEntry) -> String {
    match &v.restored_from {
        Some(f) => format!("{} (from {f})", v.operation),
        None => v.operation.clone(),
    }
}

fn current(v: &VersionEntry) -> String {
    if v.is_latest { "current".into() } else { String::new() }
}

/// A file's versions; for a folder, every version of every file in it, oldest first, each time
/// a point `restore --at` can roll the folder back to. A folder may be named without its `/`.
pub async fn history(client: &Client, out: Out, target: &Target, all: bool) -> Result<()> {
    let (drive, key) = target.resolve()?;
    let folder = if key.ends_with('/') {
        key
    } else {
        match client.list_versions(&drive, &key, all).await {
            Ok(versions) => {
                return out.emit(&json!({ "drive": drive, "key": key, "kind": "file", "versions": versions }), || {
                    let rows: Vec<Vec<String>> = versions.iter().map(|v| vec![v.version_id.clone(), v.last_modified.clone(), size(v.size), operation(v), current(v)]).collect();
                    format!("{drive}:{key}\n{}", table(&["VERSION", "TIME", "SIZE", "OPERATION", ""], &rows))
                });
            }
            Err(e) if no_such_key(&e) => {
                let folder = format!("{key}/");
                if !is_folder(client, &drive, &folder).await? {
                    return Err(missing(client, &drive, &key, e.into()).await);
                }
                folder
            }
            Err(e) => return Err(Failure::from(e).context(&key)),
        }
    };
    let listing = client.list_objects(&drive, ListOptions { prefix: Some(folder.clone()), delimiter: None }).await.map_err(|e| Failure::from(e).context(&folder))?;
    let files: Vec<String> = listing.objects.into_iter().map(|o| o.key).filter(|k| !k.ends_with('/')).collect();
    if files.is_empty() && !is_folder(client, &drive, &folder).await? {
        let f = Failure { status: Some(404), ..Failure::new("NoSuchKey", "no such file or folder") };
        return Err(missing(client, &drive, &folder, f).await);
    }
    let d = drive.as_str();
    let histories: Vec<(String, Vec<VersionEntry>)> = futures::stream::iter(files)
        .map(|k| async move {
            match client.list_versions(d, &k, all).await {
                Ok(v) => Ok((k, v)),
                // Deleted since it was listed.
                Err(e) if no_such_key(&e) => Ok((k, Vec::new())),
                Err(e) => Err(Failure::from(e).context(k)),
            }
        })
        .buffered(HISTORIES_AT_ONCE)
        .try_collect()
        .await?;
    let mut versions: Vec<InFolder> = histories.into_iter().flat_map(|(key, vs)| vs.into_iter().map(move |version| InFolder { key: key.clone(), version })).collect();
    versions.sort_by(|a, b| (&a.version.last_modified, &a.key).cmp(&(&b.version.last_modified, &b.key)));
    out.emit(&json!({ "drive": drive, "key": folder, "kind": "folder", "versions": versions }), || {
        let rows: Vec<Vec<String>> = versions
            .iter()
            .map(|f| {
                let v = &f.version;
                vec![v.last_modified.clone(), f.key[folder.len()..].to_owned(), v.version_id.clone(), size(v.size), operation(v), current(v)]
            })
            .collect();
        format!("{drive}:{folder}, every version of every file in it\n{}", table(&["TIME", "FILE", "VERSION", "SIZE", "OPERATION", ""], &rows))
    })
}

/// Where `show` writes: stdout, or a file it fills beside the target and renames into place, so
/// that a failed read leaves the target as it was.
enum Sink {
    Stdout(std::io::StdoutLock<'static>),
    File { file: std::fs::File, partial: PathBuf, path: PathBuf },
}

impl Sink {
    fn open(output: Option<&Path>) -> Result<Sink> {
        match output {
            None => Ok(Sink::Stdout(std::io::stdout().lock())),
            Some(p) if p == Path::new("-") => Ok(Sink::Stdout(std::io::stdout().lock())),
            Some(path) => {
                let name = path.file_name().ok_or_else(|| Failure::invalid(format!("-o {}: not a file name", path.display())))?;
                let partial = path.with_file_name(format!(".{}.void-{:08x}", name.to_string_lossy(), rand::random::<u32>()));
                let file = std::fs::File::create(&partial).map_err(|e| Failure::local(&partial, e))?;
                Ok(Sink::File { file, partial, path: path.to_owned() })
            }
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<()> {
        match self {
            Sink::Stdout(s) => s.write_all(data).map_err(closed),
            Sink::File { file, partial, .. } => file.write_all(data).map_err(|e| Failure::local(partial, e)),
        }
    }

    fn finish(self) -> Result<()> {
        match self {
            Sink::Stdout(mut s) => s.flush().map_err(closed),
            Sink::File { file, partial, path } => {
                file.sync_all().map_err(|e| Failure::local(&partial, e))?;
                drop(file);
                std::fs::rename(&partial, &path).map_err(|e| {
                    let _ = std::fs::remove_file(&partial);
                    Failure::local(&path, e)
                })
            }
        }
    }

    fn abandon(self) {
        if let Sink::File { partial, .. } = self {
            let _ = std::fs::remove_file(partial);
        }
    }
}

pub async fn show(client: &Client, out: Out, target: &Target, at: Option<&str>, version: Option<&str>, output: Option<&Path>) -> Result<()> {
    let (drive, key) = target.resolve()?;
    let to_file = output.filter(|p| *p != Path::new("-"));
    if out.json && to_file.is_none() {
        return Err(Failure::usage("--json needs -o <file>: without it the file's content is what goes to stdout"));
    }
    let opts = ReadOptions { version_id: version.map(str::to_owned), as_of: at.map(parse_at).transpose()? };
    let at = opts.as_of.clone();
    let mut stream = match client.get_object_stream(&drive, &key, opts).await {
        Ok(s) => s,
        Err(e) if no_such_key(&e) && version.is_none() => return Err(missing(client, &drive, &key, e.into()).await),
        Err(e) => return Err(at_context(e, &key, at.as_deref())),
    };
    let meta = stream.meta.clone();
    if meta.kind == Kind::Folder {
        return Err(Failure::invalid(format!("{key} is a folder: show prints a file. `void history {} {}` lists its versions", shell_word(&drive), shell_word(&key))));
    }
    let mut sink = Sink::open(output)?;
    let mut written = 0u64;
    loop {
        let piece = match stream.chunk().await {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => {
                sink.abandon();
                return Err(Failure::from(e).context(format!("{key}, after {written} bytes")));
            }
        };
        written += piece.len() as u64;
        if let Err(e) = sink.write(&piece) {
            sink.abandon();
            return Err(e);
        }
    }
    sink.finish()?;
    let Some(path) = to_file else { return Ok(()) };
    let summary = json!({
        "drive": drive, "key": key, "versionId": meta.version_id, "etag": meta.etag, "size": written,
        "lastModified": meta.last_modified, "mtime": meta.mtime, "output": path,
    });
    out.emit(&summary, || format!("Wrote {} to {} ({drive}:{key}, version {}).", size(written), path.display(), meta.version_id))
}

pub async fn restore(client: &Client, out: Out, target: &Target, at: Option<&str>, version: Option<&str>) -> Result<()> {
    let (drive, key) = target.resolve()?;
    let pre = voidfs_sdk::Preconditions::default();
    let (key, r, at) = match (at, version) {
        (_, Some(v)) => {
            let r = client.restore_version(&drive, &key, v, pre).await.map_err(|e| Failure::from(e).context(&key))?;
            (key, r, None)
        }
        (Some(at), None) => {
            let t = parse_at(at)?;
            let (key, r) = match client.restore_as_of(&drive, &key, &t, pre.clone()).await {
                Ok(r) => (key, r),
                // Perhaps a folder, named without its `/`.
                Err(e) if no_such_key(&e) && !key.ends_with('/') => {
                    let folder = format!("{key}/");
                    if !is_folder(client, &drive, &folder).await? {
                        return Err(missing(client, &drive, &key, e.into()).await);
                    }
                    let r = client.restore_as_of(&drive, &folder, &t, pre).await.map_err(|e| at_context(e, &folder, Some(&t)))?;
                    (folder, r)
                }
                Err(e) if no_such_key(&e) => return Err(missing(client, &drive, &key, e.into()).await),
                Err(e) => return Err(at_context(e, &key, Some(&t))),
            };
            (key, r, Some(t))
        }
        (None, None) => return Err(Failure::usage("give --at <time> or --version <id>")),
    };
    let summary = json!({ "drive": drive, "key": key, "versionId": r.version_id, "restoredFrom": r.restored_from, "at": at, "etag": r.etag, "size": r.size });
    out.emit(&summary, || {
        let from = match (&r.restored_from, &at) {
            (Some(v), _) => format!("version {v}"),
            (None, Some(t)) => format!("its state at {t}"),
            (None, None) => "an earlier version".into(),
        };
        format!("Restored {drive}:{key} to {from}, as version {}.", r.version_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_rfc_3339_or_unix_seconds() {
        assert_eq!(parse_at("2026-06-15T12:00:00Z").unwrap(), "2026-06-15T12:00:00.000000Z");
        assert_eq!(parse_at("2026-06-15T14:00:00.1234567+02:00").unwrap(), "2026-06-15T12:00:00.123456Z");
        assert_eq!(parse_at("1781524800").unwrap(), "2026-06-15T12:00:00.000000Z");
        assert_eq!(parse_at("0").unwrap(), "1970-01-01T00:00:00.000000Z");
        for bad in ["", "yesterday", "-5", "1.5", "2026-06-15", "99999999999999999999"] {
            assert_eq!(parse_at(bad).unwrap_err().code, "InvalidArgument", "{bad}");
        }
    }

    #[test]
    fn targets_take_the_drive_first_or_from_bucket() {
        let t = |first: &str, second: Option<&str>, bucket: Option<&str>| Target { first: first.into(), second: second.map(Into::into), bucket: bucket.map(Into::into) }.resolve();
        assert_eq!(t("footage", Some("/cuts/a.mov"), None).unwrap(), ("footage".into(), "cuts/a.mov".into()));
        assert_eq!(t("cuts/", None, Some("d-1")).unwrap(), ("d-1".into(), "cuts/".into()));
        assert_eq!(t("footage", None, None).unwrap_err().exit, 2);
        assert_eq!(t("footage", Some("a"), Some("b")).unwrap_err().exit, 2);
        assert_eq!(t("footage", Some("/"), None).unwrap_err().exit, 2);
    }
}
