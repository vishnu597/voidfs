// SPDX-License-Identifier: Apache-2.0
//! Object storage for a pool: the user's bucket, a local directory, or memory.
//!
//! The one operation the format depends on is [`Store::put_new`]: create an object only if it
//! does not exist, atomically (format §7.2). Buckets provide it with a conditional PUT. A local
//! directory provides it with a hard link, which fails if the target exists; OpenDAL's own
//! filesystem service checks and then renames, which is not atomic, so it is not used here.
//!
//! Garbage collection also needs every object's modification time on the store's own clock
//! (format §12), which OpenDAL's memory service does not keep; [`MemStore`] does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow};
use bytes::Bytes;
use futures::future::BoxFuture;
use opendal::{ErrorKind, Operator};
use voidfs_core::ids::Timestamp;

use crate::clock::Clock;

#[derive(Clone)]
pub enum Store {
    Local(PathBuf),
    Dal(Operator),
    Mem(Arc<MemStore>),
}

/// An object found by [`Store::list_recursive`].
#[derive(Clone, Debug)]
pub struct Listed {
    /// The path below the listed prefix.
    pub name: String,
    pub size: u64,
    /// When it was last written, on the store's clock. `None` if the store did not say.
    pub modified: Option<Timestamp>,
}

impl Store {
    pub fn memory() -> anyhow::Result<Store> {
        Ok(Store::Mem(Arc::new(MemStore::new(Clock::System))))
    }

    pub fn local(root: impl Into<PathBuf>) -> anyhow::Result<Store> {
        let root = root.into();
        std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        Ok(Store::Local(root))
    }

    /// Opens an S3-compatible bucket. With `credentials`, only those are used; without, the
    /// usual AWS sources (environment, `~/.aws`, instance metadata) are.
    pub fn s3(bucket: &str, root: &str, endpoint: Option<&str>, region: &str, credentials: Option<(&str, &str)>) -> anyhow::Result<Store> {
        let mut b = opendal::services::S3::default().bucket(bucket).root(root).region(region);
        if let Some(e) = endpoint {
            b = b.endpoint(e);
        }
        if let Some((id, secret)) = credentials {
            b = b.access_key_id(id).secret_access_key(secret).disable_config_load().disable_ec2_metadata();
        }
        Ok(Store::Dal(Operator::new(b)?.finish()))
    }

    fn local_path(root: &Path, path: &str) -> PathBuf {
        root.join(path)
    }

    pub async fn get(&self, path: &str) -> anyhow::Result<Option<Bytes>> {
        match self {
            Store::Local(root) => match tokio::fs::read(Self::local_path(root, path)).await {
                Ok(v) => Ok(Some(Bytes::from(v))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("reading {path}")),
            },
            Store::Dal(op) => match op.read(path).await {
                Ok(b) => Ok(Some(b.to_bytes())),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("reading {path}")),
            },
            Store::Mem(m) => m.run(MemOp::Get, path, |o| Ok(o.get(path).map(|(b, _)| b.clone()))).await,
        }
    }

    /// When the object was last written, on the store's clock, or `None` if it does not exist.
    pub async fn modified(&self, path: &str) -> anyhow::Result<Option<Timestamp>> {
        match self {
            Store::Local(root) => match tokio::fs::metadata(Self::local_path(root, path)).await {
                Ok(m) => Ok(Some(Timestamp::from_datetime(m.modified()?.into()))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("reading the metadata of {path}")),
            },
            Store::Dal(op) => match op.stat(path).await {
                Ok(m) => Ok(Some(dal_time(m.last_modified()).ok_or_else(|| anyhow!("the store gave no modification time for {path}"))?)),
                Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("reading the metadata of {path}")),
            },
            Store::Mem(m) => m.run(MemOp::Head, path, |o| Ok(o.get(path).map(|(_, t)| *t))).await,
        }
    }

    pub async fn exists(&self, path: &str) -> anyhow::Result<bool> {
        match self {
            Store::Local(root) => Ok(tokio::fs::try_exists(Self::local_path(root, path)).await?),
            Store::Dal(op) => Ok(op.exists(path).await?),
            Store::Mem(m) => m.run(MemOp::Head, path, |o| Ok(o.contains_key(path))).await,
        }
    }

    /// Writes (or overwrites) an object.
    pub async fn put(&self, path: &str, data: Bytes) -> anyhow::Result<()> {
        match self {
            Store::Local(root) => {
                let target = Self::local_path(root, path);
                let tmp = Self::write_temp(root, &target, &data).await?;
                tokio::fs::rename(&tmp, &target).await.with_context(|| format!("writing {path}"))?;
                Ok(())
            }
            Store::Dal(op) => {
                op.write(path, data).await.with_context(|| format!("writing {path}"))?;
                Ok(())
            }
            Store::Mem(m) => {
                let now = m.clock.now();
                m.run(MemOp::Put, path, |o| {
                    o.insert(path.to_owned(), (data, now));
                    Ok(())
                })
                .await
            }
        }
    }

    /// Creates an object only if it does not exist. Returns `false` if it already did.
    pub async fn put_new(&self, path: &str, data: Bytes) -> anyhow::Result<bool> {
        match self {
            Store::Local(root) => {
                let target = Self::local_path(root, path);
                let tmp = Self::write_temp(root, &target, &data).await?;
                let linked = tokio::fs::hard_link(&tmp, &target).await;
                let _ = tokio::fs::remove_file(&tmp).await;
                match linked {
                    Ok(()) => Ok(true),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
                    Err(e) => Err(e).with_context(|| format!("creating {path}")),
                }
            }
            Store::Dal(op) => match op.write_with(path, data).if_not_exists(true).await {
                Ok(_) => Ok(true),
                Err(e) if e.kind() == ErrorKind::ConditionNotMatch => Ok(false),
                Err(e) => Err(e).with_context(|| format!("creating {path}")),
            },
            Store::Mem(m) => {
                let now = m.clock.now();
                m.run_faulted(MemOp::PutNew, path, |o, fault| {
                    if o.contains_key(path) && fault != Fault::Unconditional {
                        return Ok(false);
                    }
                    o.insert(path.to_owned(), (data, now));
                    Ok(true)
                })
                .await
            }
        }
    }

    async fn write_temp(root: &Path, target: &Path, data: &[u8]) -> anyhow::Result<PathBuf> {
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp_dir = root.join(".tmp");
        tokio::fs::create_dir_all(&tmp_dir).await?;
        let tmp = tmp_dir.join(uuid::Uuid::new_v4().to_string());
        let mut f = tokio::fs::File::create(&tmp).await?;
        use tokio::io::AsyncWriteExt;
        f.write_all(data).await?;
        f.sync_all().await?;
        Ok(tmp)
    }

    pub async fn delete(&self, path: &str) -> anyhow::Result<()> {
        match self {
            Store::Local(root) => match tokio::fs::remove_file(Self::local_path(root, path)).await {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
            Store::Dal(op) => Ok(op.delete(path).await?),
            Store::Mem(m) => {
                m.run(MemOp::Delete, path, |o| {
                    o.remove(path);
                    Ok(())
                })
                .await
            }
        }
    }

    /// Deletes every object under `prefix` (which ends in `/`).
    pub async fn delete_prefix(&self, prefix: &str) -> anyhow::Result<()> {
        match self {
            Store::Local(root) => match tokio::fs::remove_dir_all(Self::local_path(root, prefix)).await {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
            Store::Dal(op) => Ok(op.delete_with(prefix).recursive(true).await?),
            Store::Mem(m) => {
                m.run(MemOp::Delete, prefix, |o| {
                    o.retain(|k, _| !k.starts_with(prefix));
                    Ok(())
                })
                .await
            }
        }
    }

    /// Every object under `prefix` (which ends in `/`), at any depth, sorted by name.
    pub async fn list_recursive(&self, prefix: &str) -> anyhow::Result<Vec<Listed>> {
        let mut out = Vec::new();
        match self {
            Store::Local(root) => {
                let base = Self::local_path(root, prefix);
                let mut dirs = vec![base.clone()];
                while let Some(dir) = dirs.pop() {
                    let mut rd = match tokio::fs::read_dir(&dir).await {
                        Ok(rd) => rd,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(e) => return Err(e.into()),
                    };
                    while let Some(entry) = rd.next_entry().await? {
                        let meta = entry.metadata().await?;
                        if meta.is_dir() {
                            dirs.push(entry.path());
                        } else {
                            let rel = entry.path().strip_prefix(&base)?.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
                            out.push(Listed { name: rel, size: meta.len(), modified: Some(Timestamp::from_datetime(meta.modified()?.into())) });
                        }
                    }
                }
            }
            Store::Dal(op) => {
                let entries = match op.list_with(prefix).recursive(true).await {
                    Ok(e) => e,
                    Err(e) if e.kind() == ErrorKind::NotFound => return Ok(out),
                    Err(e) => return Err(e.into()),
                };
                for e in entries {
                    let Some(name) = e.path().strip_prefix(prefix) else { continue };
                    if name.is_empty() || name.ends_with('/') {
                        continue;
                    }
                    let m = e.metadata();
                    out.push(Listed { name: name.to_owned(), size: m.content_length(), modified: dal_time(m.last_modified()) });
                }
            }
            Store::Mem(m) => {
                out = m
                    .run(MemOp::List, prefix, |o| {
                        Ok(o.range(prefix.to_owned()..)
                            .take_while(|(k, _)| k.starts_with(prefix))
                            .map(|(k, (b, t))| Listed { name: k[prefix.len()..].to_owned(), size: b.len() as u64, modified: Some(*t) })
                            .collect())
                    })
                    .await?;
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Names of the objects directly under `prefix` (which ends in `/`), sorted, after
    /// `start_after` if given. Only objects, not sub-prefixes.
    pub async fn list_files(&self, prefix: &str, start_after: Option<&str>) -> anyhow::Result<Vec<String>> {
        let mut out = self.list(prefix, false).await?;
        out.retain(|n| !n.ends_with('/') && start_after.is_none_or(|a| n.as_str() > a));
        Ok(out)
    }

    /// Names of the sub-prefixes directly under `prefix`, without the trailing `/`.
    pub async fn list_dirs(&self, prefix: &str) -> anyhow::Result<Vec<String>> {
        let out = self.list(prefix, true).await?;
        Ok(out.into_iter().filter_map(|n| n.strip_suffix('/').map(str::to_owned)).collect())
    }

    /// Direct children of `prefix`: file names, and dir names with a trailing `/`.
    async fn list(&self, prefix: &str, dirs: bool) -> anyhow::Result<Vec<String>> {
        let mut out = Vec::new();
        match self {
            Store::Local(root) => {
                let mut rd = match tokio::fs::read_dir(Self::local_path(root, prefix)).await {
                    Ok(rd) => rd,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
                    Err(e) => return Err(e.into()),
                };
                while let Some(entry) = rd.next_entry().await? {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if entry.file_type().await?.is_dir() {
                        if dirs {
                            out.push(format!("{name}/"));
                        }
                    } else if !dirs {
                        out.push(name);
                    }
                }
            }
            Store::Mem(m) => {
                out = m
                    .run(MemOp::List, prefix, |o| {
                        let mut names: Vec<String> = Vec::new();
                        for (k, _) in o.range(prefix.to_owned()..).take_while(|(k, _)| k.starts_with(prefix)) {
                            let rest = &k[prefix.len()..];
                            let name = match rest.find('/') {
                                Some(i) if dirs => format!("{}/", &rest[..i]),
                                Some(_) => continue,
                                None if dirs => continue,
                                None => rest.to_owned(),
                            };
                            if names.last() != Some(&name) {
                                names.push(name);
                            }
                        }
                        Ok(names)
                    })
                    .await?;
            }
            Store::Dal(op) => {
                let entries = match op.list(prefix).await {
                    Ok(e) => e,
                    Err(e) if e.kind() == ErrorKind::NotFound => return Ok(out),
                    Err(e) => return Err(e.into()),
                };
                for e in entries {
                    let Some(name) = e.path().strip_prefix(prefix) else { continue };
                    if name.is_empty() {
                        continue;
                    }
                    if name.ends_with('/') == dirs {
                        out.push(name.to_owned());
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

fn dal_time(t: Option<opendal::raw::Timestamp>) -> Option<Timestamp> {
    let micros = t?.into_inner().as_microsecond();
    chrono::DateTime::from_timestamp_micros(micros).map(Timestamp::from_datetime)
}

/// What a request to a [`MemStore`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemOp {
    Get,
    Head,
    Put,
    PutNew,
    Delete,
    List,
}

/// What a [`Hook`] makes a request do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    None,
    /// Fail without doing anything.
    Fail,
    /// Do it, then report a failure: the reply was lost.
    FailAfter,
    /// A create-if-absent write overwrites like a plain one, as on a backend that ignores
    /// `If-None-Match`. Other requests are unaffected.
    Unconditional,
}

/// Runs before every request to a [`MemStore`]: it can delay the request (to reorder requests
/// in simulations) and make it fail.
pub type Hook = Arc<dyn Fn(MemOp, &str) -> BoxFuture<'static, Fault> + Send + Sync>;

/// Objects in memory, with modification times from a [`Clock`].
pub struct MemStore {
    objects: Mutex<BTreeMap<String, (Bytes, Timestamp)>>,
    pub clock: Clock,
    hook: Mutex<Option<Hook>>,
}

impl MemStore {
    pub fn new(clock: Clock) -> MemStore {
        MemStore { objects: Mutex::new(BTreeMap::new()), clock, hook: Mutex::new(None) }
    }

    #[cfg(test)]
    pub fn set_hook(&self, hook: Option<Hook>) {
        *self.hook.lock().unwrap() = hook;
    }

    /// Reads an object without going through the hook.
    #[cfg(test)]
    pub fn peek(&self, path: &str) -> Option<Bytes> {
        self.objects.lock().unwrap().get(path).map(|(b, _)| b.clone())
    }

    async fn run<T>(&self, op: MemOp, path: &str, f: impl FnOnce(&mut BTreeMap<String, (Bytes, Timestamp)>) -> anyhow::Result<T>) -> anyhow::Result<T> {
        self.run_faulted(op, path, |o, _| f(o)).await
    }

    /// [`MemStore::run`], telling `f` the fault the hook chose.
    async fn run_faulted<T>(&self, op: MemOp, path: &str, f: impl FnOnce(&mut BTreeMap<String, (Bytes, Timestamp)>, Fault) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let hook = self.hook.lock().unwrap().clone();
        let fault = match hook {
            Some(h) => h(op, path).await,
            None => Fault::None,
        };
        if fault == Fault::Fail {
            return Err(anyhow!("injected failure: {op:?} {path}"));
        }
        let out = f(&mut self.objects.lock().unwrap(), fault)?;
        if fault == Fault::FailAfter {
            return Err(anyhow!("injected failure after {op:?} {path}"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn exercise(s: Store) {
        assert!(s.get("a/b.json").await.unwrap().is_none());
        assert!(s.put_new("a/b.json", Bytes::from_static(b"1")).await.unwrap());
        assert!(!s.put_new("a/b.json", Bytes::from_static(b"2")).await.unwrap());
        assert_eq!(s.get("a/b.json").await.unwrap().unwrap(), Bytes::from_static(b"1"));
        s.put("a/c.json", Bytes::from_static(b"3")).await.unwrap();
        s.put("a/d/e", Bytes::from_static(b"4")).await.unwrap();
        assert_eq!(s.list_files("a/", None).await.unwrap(), ["b.json", "c.json"]);
        assert_eq!(s.list_files("a/", Some("b.json")).await.unwrap(), ["c.json"]);
        assert_eq!(s.list_dirs("a/").await.unwrap(), ["d"]);
        assert!(s.list_files("nope/", None).await.unwrap().is_empty());
        let all = s.list_recursive("a/").await.unwrap();
        assert_eq!(all.iter().map(|l| (l.name.as_str(), l.size)).collect::<Vec<_>>(), [("b.json", 1), ("c.json", 1), ("d/e", 1)]);
        assert!(all.iter().all(|l| l.modified.is_some()));
        assert_eq!(s.modified("a/d/e").await.unwrap(), all[2].modified);
        assert!(s.modified("a/nope").await.unwrap().is_none());
        assert!(s.list_recursive("nope/").await.unwrap().is_empty());
        s.delete("a/c.json").await.unwrap();
        s.delete("a/c.json").await.unwrap();
        s.delete_prefix("a/").await.unwrap();
        assert!(s.get("a/b.json").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn memory_store() {
        exercise(Store::memory().unwrap()).await;
    }

    #[tokio::test]
    async fn local_store() {
        let dir = std::env::temp_dir().join(format!("voidfs-store-{}", uuid::Uuid::new_v4()));
        exercise(Store::local(&dir).unwrap()).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A bucket named by the `VOIDFS_S3_*` variables, under a fresh prefix. Run with
    /// `cargo test -p voidfs-server -- --ignored` after loading `.env`.
    fn s3_from_env() -> Store {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} is not set"));
        let root = format!("/voidfs-test-{}/store/", chrono::Utc::now().format("%Y%m%dT%H%M%S"));
        let id = var("VOIDFS_S3_ACCESS_KEY_ID");
        let secret = var("VOIDFS_S3_SECRET_ACCESS_KEY");
        let region = std::env::var("VOIDFS_S3_REGION").unwrap_or_else(|_| "auto".into());
        Store::s3(&var("VOIDFS_S3_BUCKET"), &root, Some(&var("VOIDFS_S3_ENDPOINT")), &region, Some((&id, &secret))).unwrap()
    }

    #[tokio::test]
    #[ignore = "needs a real bucket"]
    async fn s3_store() {
        exercise(s3_from_env()).await;
    }

    #[tokio::test]
    #[ignore = "needs a real bucket"]
    async fn s3_put_new_is_exclusive_under_races() {
        races(s3_from_env()).await;
    }

    async fn races(s: Store) {
        let tasks: Vec<_> = (0..32)
            .map(|i| {
                let s = s.clone();
                tokio::spawn(async move { s.put_new("log/1.json", Bytes::from(format!("{i}"))).await.unwrap() })
            })
            .collect();
        let mut wins = 0;
        for t in tasks {
            wins += t.await.unwrap() as usize;
        }
        assert_eq!(wins, 1, "exactly one racing writer may create the object");
        s.delete("log/1.json").await.unwrap();
    }

    #[tokio::test]
    async fn local_put_new_is_exclusive_under_races() {
        let dir = std::env::temp_dir().join(format!("voidfs-race-{}", uuid::Uuid::new_v4()));
        let s = Store::local(&dir).unwrap();
        let tasks: Vec<_> = (0..32)
            .map(|i| {
                let s = s.clone();
                tokio::spawn(async move { s.put_new("log/1.json", Bytes::from(format!("{i}"))).await.unwrap() })
            })
            .collect();
        let mut wins = 0;
        for t in tasks {
            wins += t.await.unwrap() as usize;
        }
        assert_eq!(wins, 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
