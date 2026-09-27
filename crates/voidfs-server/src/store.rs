// SPDX-License-Identifier: Apache-2.0
//! Object storage for a pool: the user's bucket, a local directory, or memory.
//!
//! The one operation the format depends on is [`Store::put_new`]: create an object only if it
//! does not exist, atomically (format §7.2). Buckets provide it with a conditional PUT. A local
//! directory provides it with a hard link, which fails if the target exists; OpenDAL's own
//! filesystem service checks and then renames, which is not atomic, so it is not used here.

use std::path::{Path, PathBuf};

use anyhow::Context;
use bytes::Bytes;
use opendal::{ErrorKind, Operator};

#[derive(Clone)]
pub enum Store {
    Local(PathBuf),
    Dal(Operator),
}

impl Store {
    pub fn memory() -> anyhow::Result<Store> {
        Ok(Store::Dal(Operator::new(opendal::services::Memory::default())?.finish()))
    }

    pub fn local(root: impl Into<PathBuf>) -> anyhow::Result<Store> {
        let root = root.into();
        std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        Ok(Store::Local(root))
    }

    pub fn s3(bucket: &str, root: &str, endpoint: Option<&str>, region: &str) -> anyhow::Result<Store> {
        let mut b = opendal::services::S3::default().bucket(bucket).root(root).region(region);
        if let Some(e) = endpoint {
            b = b.endpoint(e);
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
        }
    }

    pub async fn exists(&self, path: &str) -> anyhow::Result<bool> {
        match self {
            Store::Local(root) => Ok(tokio::fs::try_exists(Self::local_path(root, path)).await?),
            Store::Dal(op) => Ok(op.exists(path).await?),
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
        }
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
