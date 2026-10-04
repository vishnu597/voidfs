// SPDX-License-Identifier: Apache-2.0
//! The reader against pools the server wrote: it must build every drive's state as the server
//! has it, from the bucket alone.

use bytes::Bytes;
use futures::future::BoxFuture;
use voidfs_core::ids::{DriveId, ShardHash};
use voidfs_core::manifest;
use voidfs_core::model::{ContentDescriptor, Extent, MULTI_OBJECT_VERSIONS};
use voidfs_format::Source;
use voidfs_sdk::{Client, Config};
use voidfs_server::pool::Pool;
use voidfs_server::store::Store;
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, TestServer};

/// The pool's store, read as a client reads a bucket: no caches, pages checked by the reader.
struct Bucket(Store);

impl Source for Bucket {
    fn get<'a>(&'a self, path: &'a str) -> BoxFuture<'a, anyhow::Result<Option<Bytes>>> {
        Box::pin(self.0.get(path))
    }

    fn list<'a>(&'a self, dir: &'a str, after: Option<&'a str>) -> BoxFuture<'a, anyhow::Result<Vec<String>>> {
        Box::pin(self.0.list_files(dir, after))
    }
}

async fn setup() -> (TestServer, Client, Bucket) {
    let s = TestServer::start().await.unwrap();
    let c = Client::new(Config { endpoint: s.endpoint.clone(), access_key_id: ADMIN_KEY_ID.into(), secret_access_key: ADMIN_SECRET.into(), ..Config::default() }).unwrap();
    let b = Bucket(s.pool.store.clone());
    (s, c, b)
}

fn id_of(s: &TestServer, name: &str) -> DriveId {
    s.pool.drive(name).unwrap().id.clone()
}

/// The drive as the reader builds it from the bucket, checked to match `pool`'s.
async fn same_as(b: &Bucket, pool: &Pool, name: &str) {
    let d = pool.drive(name).unwrap();
    let desc = voidfs_format::open_pool(b).await.unwrap();
    let (drive, state) = voidfs_format::load_drive(b, &desc, &d.id).await.unwrap().unwrap();
    assert_eq!(drive, d.desc);
    let want = d.snapshot();
    assert_eq!(state.seq(), want.seq(), "{name}");
    assert_eq!(state.rows(), want.rows(), "{name}");
    // And all at once, as a client far from the bucket reads it.
    let (pool, drive, state) = voidfs_format::open_drive(b, &d.id).await.unwrap().unwrap();
    assert_eq!((pool, drive), (desc, d.desc.clone()));
    assert_eq!(state.seq(), want.seq(), "{name}, at once");
    assert_eq!(state.rows(), want.rows(), "{name}, at once");
}

#[tokio::test(flavor = "multi_thread")]
async fn drives_are_read_as_the_server_has_them() {
    let (s, c, b) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    c.put_object("drv", "a.txt", "one", Default::default()).await.unwrap();
    c.put_object("drv", "docs/b.txt", vec![7u8; 300 << 10], Default::default()).await.unwrap();
    c.put_object("drv", "a.txt", "two", Default::default()).await.unwrap();
    c.rename("drv", "docs/b.txt", "docs/c.txt", Default::default()).await.unwrap();
    c.write_at("drv", "docs/c.txt", 1000, "edit", Default::default()).await.unwrap();
    // A fork starts from a checkpoint of its own (format §9), and its log follows it.
    c.fork_drive("drv", "fork").await.unwrap();
    c.put_object("fork", "f.txt", "in the fork", Default::default()).await.unwrap();
    c.delete_object("fork", "a.txt", Default::default()).await.unwrap();
    c.put_object("drv", "after.txt", "after the fork", Default::default()).await.unwrap();
    assert!(voidfs_format::latest_checkpoint(&b, &id_of(&s, "fork"), false).await.unwrap().is_some(), "the fork has a checkpoint");
    same_as(&b, &s.pool, "drv").await;
    same_as(&b, &s.pool, "fork").await;
    assert!(voidfs_format::load_drive(&b, &voidfs_format::open_pool(&b).await.unwrap(), &DriveId::generate()).await.unwrap().is_none(), "no such drive");
    assert!(voidfs_format::open_drive(&b, &DriveId::generate()).await.unwrap().is_none(), "no such drive");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pool_says_how_versions_are_read() {
    let (s, c, b) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    c.put_object("drv", "docs/a", "first", Default::default()).await.unwrap();
    let at = c.list_versions("drv", "docs/a", false).await.unwrap()[0].last_modified.clone();
    c.put_object("drv", "docs/a", "second", Default::default()).await.unwrap();
    c.put_object("drv", "docs/new", "new", Default::default()).await.unwrap();
    c.restore_as_of("drv", "docs/", &at, Default::default()).await.unwrap();
    same_as(&b, &s.pool, "drv").await;
    let before = s.pool.drive("drv").unwrap().snapshot();

    // With multi-object-versions, the folder restore is a version of each file it changes
    // (format §7.5): a reader that read the pool without it would give them other heads.
    assert!(voidfs_server::pool::enable_feature(&s.pool.store, MULTI_OBJECT_VERSIONS).await.unwrap());
    let again = Pool::open(s.pool.store.clone(), 1 << 20).await.unwrap();
    assert_ne!(again.drive("drv").unwrap().snapshot().rows(), before.rows(), "the feature changes the state");
    same_as(&b, &again, "drv").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pools_it_cannot_read_are_refused() {
    let b = Bucket(Store::memory().unwrap());
    let e = voidfs_format::open_pool(&b).await.unwrap_err();
    assert!(format!("{e:#}").contains("no pool"), "{e:#}");
    let pool = |format: u32, incompatible: &str| {
        format!(r#"{{"format":{format},"pool_id":"p-1","created":"2026-10-04T00:00:00Z","features":{{"compatible":["unknown-compatible"],"incompatible":[{incompatible}]}},"chunking":{{"algorithm":"fastcdc-2020","min":262144,"avg":2097152,"max":16777216}},"hash":"sha256","commit_guard":"create-if-absent"}}"#)
    };
    b.0.put("voidfs.json", pool(1, r#""inline-data","multi-object-versions""#).into()).await.unwrap();
    assert!(voidfs_format::open_pool(&b).await.unwrap().has(MULTI_OBJECT_VERSIONS));
    b.0.put("voidfs.json", pool(1, r#""shard-zstd""#).into()).await.unwrap();
    let e = voidfs_format::open_pool(&b).await.unwrap_err();
    assert!(format!("{e:#}").contains("shard-zstd"), "{e:#}");
    b.0.put("voidfs.json", pool(2, "").into()).await.unwrap();
    let e = voidfs_format::open_pool(&b).await.unwrap_err();
    assert!(format!("{e:#}").contains("format 2"), "{e:#}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_is_read_up_to_its_first_gap() {
    let (s, c, b) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    for i in 0..4 {
        c.put_object("drv", &format!("f{i}"), "x", Default::default()).await.unwrap();
    }
    let id = id_of(&s, "drv");
    let desc = voidfs_format::open_pool(&b).await.unwrap();
    let (_, all) = voidfs_format::load_drive(&b, &desc, &id).await.unwrap().unwrap();
    assert_eq!(all.seq(), 4);
    // A commit still being written, or lost: readers stop before it (format §8.4).
    s.pool.store.delete(&voidfs_format::log_path(&id, 3)).await.unwrap();
    let (_, state) = voidfs_format::load_drive(&b, &desc, &id).await.unwrap().unwrap();
    assert_eq!(state.seq(), 2);
    assert_eq!(voidfs_format::open_drive(&b, &id).await.unwrap().unwrap().2.seq(), 2);
    let mut caught = state.clone();
    assert_eq!(voidfs_format::catch_up(&b, &id, &mut caught).await.unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_that_does_not_match_its_hash_is_refused() {
    let (s, c, b) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    c.put_object("drv", "a", "x", Default::default()).await.unwrap();
    c.fork_drive("drv", "fork").await.unwrap();
    let id = id_of(&s, "fork");
    let c = voidfs_format::latest_checkpoint(&b, &id, false).await.unwrap().unwrap();
    let page = c.index.tables.entries[0].page;
    // Rows that parse, under the hash of other rows.
    let bytes = s.pool.store.get(&voidfs_format::page_path(&page)).await.unwrap().unwrap();
    let changed = String::from_utf8(bytes.to_vec()).unwrap().replace("\"a\"", "\"b\"");
    assert_ne!(changed.as_bytes(), &bytes[..]);
    s.pool.store.put(&voidfs_format::page_path(&page), changed.into()).await.unwrap();
    let e = voidfs_format::latest_checkpoint(&b, &id, false).await.err().expect("a corrupt segment loaded");
    assert!(format!("{e:#}").contains("corrupt"), "{e:#}");
}

#[tokio::test(flavor = "multi_thread")]
async fn manifest_trees_are_flattened() {
    let b = Bucket(Store::memory().unwrap());
    let extents: Vec<Extent> = (0..3000u32).map(|i| Extent::Shard { s: ShardHash::of(&i.to_be_bytes()), n: 1000 + u64::from(i % 7) }).collect();
    let (desc, pages) = manifest::describe(extents.clone());
    assert!(matches!(desc, ContentDescriptor::Tree { .. }) && pages.len() > 2, "a tree of several pages");
    for p in &pages {
        b.0.put(&voidfs_format::page_path(&p.hash), p.bytes.clone()).await.unwrap();
    }
    assert_eq!(voidfs_format::extents(&b, &desc).await.unwrap(), extents);
    let small = ContentDescriptor::Inline { extents: extents[..3].to_vec() };
    assert_eq!(voidfs_format::extents(&b, &small).await.unwrap(), &extents[..3]);
    b.0.delete(&voidfs_format::page_path(&pages[0].hash)).await.unwrap();
    let e = voidfs_format::extents(&b, &desc).await.unwrap_err();
    assert!(format!("{e:#}").contains("missing"), "{e:#}");
}
