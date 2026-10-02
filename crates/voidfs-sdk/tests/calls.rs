// SPDX-License-Identifier: Apache-2.0
//! Every call of the SDK against a server in this process.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use common::setup;
use voidfs_sdk::*;
use voidfs_server::sigv4::{KeyInfo, Scope};
use voidfs_server::test_server::TestServer;

async fn text(c: &Client, drive: &str, key: &str) -> String {
    String::from_utf8(c.get_object(drive, key, Default::default()).await.unwrap().body.to_vec()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn drives_are_created_described_forked_deleted_and_recovered() {
    let (_s, c) = setup().await;
    let made = c.create_drive("footage", CreateDrive { display_name: Some("Footage & co".into()) }).await.unwrap();
    assert!(made.drive_id.starts_with("d-"), "{made:?}");
    let names: Vec<String> = c.list_drives().await.unwrap().into_iter().map(|d| d.name).collect();
    assert_eq!(names, ["footage"]);
    let info = c.describe_drive("footage").await.unwrap();
    assert_eq!((info.drive_id.as_str(), info.alias.as_str(), info.fork_of.as_ref()), (made.drive_id.as_str(), "footage", None));

    c.put_object("footage", "a.txt", "hello", Default::default()).await.unwrap();
    let fork = c.fork_drive("footage", "footage-try").await.unwrap();
    assert_eq!(fork.source_id.as_deref(), Some(made.drive_id.as_str()));
    assert!(fork.fork_point.is_some());
    c.put_object("footage-try", "a.txt", "changed", Default::default()).await.unwrap();
    assert_eq!(text(&c, "footage", "a.txt").await, "hello", "a fork's writes stay in the fork");
    let info = c.describe_drive("footage-try").await.unwrap();
    assert_eq!(info.fork_of.map(|f| f.drive_id), Some(made.drive_id.clone()));
    assert_eq!(c.describe_drive("footage").await.unwrap().forks, std::slice::from_ref(&fork.drive_id));

    let e = c.create_drive("footage", Default::default()).await.unwrap_err();
    assert_eq!((e.status(), e.code()), (Some(409), Some("BucketAlreadyOwnedByYou")));
    let e = c.fork_drive("nowhere", "other").await.unwrap_err();
    assert_eq!(e.code(), Some("NoSuchBucket"));

    c.delete_drive("footage-try", false).await.unwrap();
    assert!(c.describe_drive("footage-try").await.unwrap_err().is_not_found());
    assert_eq!(c.undelete_drive("footage-try").await.unwrap().drive_id, fork.drive_id);
    assert_eq!(text(&c, "footage-try", "a.txt").await, "changed");
    c.delete_drive("footage-try", true).await.unwrap();
    assert!(c.undelete_drive("footage-try").await.unwrap_err().is_not_found());
    let names: Vec<String> = c.list_drives().await.unwrap().into_iter().map(|d| d.name).collect();
    assert_eq!(names, ["footage"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn objects_are_written_read_by_range_and_deleted() {
    let (_s, c) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    let opts = PutOptions {
        content_type: Some("text/plain".into()),
        mtime: Some("2026-09-01T10:00:00.000000Z".into()),
        mode: Some(0o640),
        metadata: BTreeMap::from([("colour".to_string(), "blue".to_string())]),
        ..Default::default()
    };
    let w = c.put_object("drv", "dir/a b+é.txt", "0123456789", opts).await.unwrap();
    assert_eq!(w.size, Some(10));
    assert!(w.etag.is_some());
    let o = c.get_object("drv", "dir/a b+é.txt", Default::default()).await.unwrap();
    assert_eq!(&o.body[..], b"0123456789");
    let m = &o.meta;
    assert_eq!((m.version_id.as_str(), m.size, m.kind), (w.version_id.as_str(), 10, Kind::File));
    assert_eq!((m.content_type.as_deref(), m.mode.as_deref(), m.mtime.as_deref()), (Some("text/plain"), Some("0640"), Some("2026-09-01T10:00:00.000000Z")));
    assert_eq!(m.metadata.get("colour").map(String::as_str), Some("blue"));
    assert!(m.object_id.as_deref().is_some_and(|id| id.starts_with("o-")));
    assert_eq!(c.head_object("drv", "dir/a b+é.txt", Default::default()).await.unwrap(), o.meta);
    assert_eq!(c.head_object("drv", "dir/", Default::default()).await.unwrap().kind, Kind::Folder, "the parent folder was made");

    assert_eq!(&c.read_range("drv", "dir/a b+é.txt", 2, Some(3), Default::default()).await.unwrap()[..], b"234");
    assert_eq!(&c.read_range("drv", "dir/a b+é.txt", 7, None, Default::default()).await.unwrap()[..], b"789");
    assert_eq!(&c.read_range("drv", "dir/a b+é.txt", 8, Some(100), Default::default()).await.unwrap()[..], b"89");
    assert!(c.read_range("drv", "dir/a b+é.txt", 3, Some(0), Default::default()).await.unwrap().is_empty());
    assert_eq!(c.read_range("drv", "dir/a b+é.txt", 10, None, Default::default()).await.unwrap_err().status(), Some(416));

    let mut s = c.get_object_stream("drv", "dir/a b+é.txt", Default::default()).await.unwrap();
    assert_eq!(s.meta.size, 10);
    assert_eq!(&s.chunk().await.unwrap().unwrap()[..], b"0123456789");
    assert_eq!(s.chunk().await.unwrap(), None);

    let big = Bytes::from((0..5_000_000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>());
    c.put_object("drv", "big", big.clone(), Default::default()).await.unwrap();
    let streamed = c.get_object_stream("drv", "big", Default::default()).await.unwrap().collect().await.unwrap();
    assert_eq!(streamed, big);

    let v = c.delete_object("drv", "dir/a b+é.txt", Default::default()).await.unwrap();
    assert!(v.is_some());
    let e = c.get_object("drv", "dir/a b+é.txt", Default::default()).await.unwrap_err();
    assert_eq!((e.status(), e.code()), (Some(404), Some("NoSuchKey")));
    assert!(e.request_id().is_some());
    assert_eq!(c.head_object("drv", "gone", Default::default()).await.unwrap_err().code(), Some("NotFound"), "HEAD has no body");
    assert_eq!(c.delete_object("drv", "never", Default::default()).await.unwrap(), None);
    assert!(matches!(c.get_object("drv", "", Default::default()).await, Err(Error::Invalid(_))));
}

#[tokio::test(flavor = "multi_thread")]
async fn preconditions_fail_with_the_current_version() {
    let (_s, c) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    let only_new = PutOptions { if_none_match_any: true, ..Default::default() };
    let v1 = c.put_object("drv", "k", "one", only_new.clone()).await.unwrap();
    let e = c.put_object("drv", "k", "two", only_new).await.unwrap_err();
    assert_eq!((e.status(), e.current_version_id()), (Some(412), Some(v1.version_id.as_str())));

    let v2 = c.write_at("drv", "k", 0, "ONE", WriteOptions { if_version: Some(v1.version_id.clone()), ..Default::default() }).await.unwrap();
    let e = c.write_at("drv", "k", 0, "xxx", WriteOptions { if_version: Some(v1.version_id.clone()), ..Default::default() }).await.unwrap_err();
    assert_eq!((e.code(), e.current_version_id()), (Some("PreconditionFailed"), Some(v2.version_id.as_str())));
    let e = c.splice("drv", "k", 0, 1, Bytes::new(), Preconditions { if_match: Some("\"stale\"".into()), ..Default::default() }).await.unwrap_err();
    assert_eq!(e.status(), Some(412));
    c.put_object("drv", "k", "three", PutOptions { if_match: v2.etag.clone(), ..Default::default() }).await.unwrap();
    let e = c.delete_object("drv", "k", Preconditions::if_version(v2.version_id.clone())).await.unwrap_err();
    assert_eq!(e.status(), Some(412));
    assert_eq!(text(&c, "drv", "k").await, "three");
}

#[tokio::test(flavor = "multi_thread")]
async fn files_are_edited_in_place() {
    let (_s, c) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    let w = c.write_at("drv", "f", 3, "abc", Default::default()).await.unwrap();
    assert_eq!(w.size, Some(6));
    assert_eq!(text(&c, "drv", "f").await, "\0\0\0abc", "created with zeros before the offset");
    c.write_at("drv", "f", 0, "XY", WriteOptions { size: Some(4), mode: Some(0o600), ..Default::default() }).await.unwrap();
    assert_eq!(text(&c, "drv", "f").await, "XY\0a");
    assert_eq!(c.head_object("drv", "f", Default::default()).await.unwrap().mode.as_deref(), Some("0600"));
    assert_eq!(c.truncate("drv", "f", 2, Default::default()).await.unwrap().size, Some(2));
    assert_eq!(c.truncate("drv", "f", 5, Default::default()).await.unwrap().size, Some(5));
    assert_eq!(text(&c, "drv", "f").await, "XY\0\0\0");

    c.put_object("drv", "p", "hello world", Default::default()).await.unwrap();
    c.patch("drv", "p", &[Edit::new(0, "H"), Edit::new(6, "W"), Edit::new(11, "!"), Edit::new(0, "J")], Default::default()).await.unwrap();
    assert_eq!(text(&c, "drv", "p").await, "Jello World!", "in order, the later edit winning");
    assert!(matches!(c.patch("drv", "p", &[], Default::default()).await, Err(Error::Invalid(_))));
    let e = c.raw_patch("drv", "p", "not a patch", Default::default()).await.unwrap_err();
    assert_eq!((e.status(), e.code()), (Some(400), Some("InvalidPatch")));

    let head = c.head_object("drv", "p", Default::default()).await.unwrap();
    c.insert("drv", "p", 5, ",", Preconditions::if_version(head.version_id)).await.unwrap();
    assert_eq!(text(&c, "drv", "p").await, "Jello, World!");
    c.remove_range("drv", "p", 0, 7, Default::default()).await.unwrap();
    assert_eq!(text(&c, "drv", "p").await, "World!");
    let w = c.splice("drv", "p", 0, 5, "Earth", Default::default()).await.unwrap();
    assert_eq!(w.size, Some(6));
    assert_eq!(text(&c, "drv", "p").await, "Earth!");
    let e = c.remove_range("drv", "p", 4, 10, Default::default()).await.unwrap_err();
    assert_eq!(e.code(), Some("InvalidArgument"));
    assert_eq!(c.insert("drv", "absent", 0, "x", Default::default()).await.unwrap_err().code(), Some("NoSuchKey"));
}

#[tokio::test(flavor = "multi_thread")]
async fn renames_move_files_and_folders_with_their_history() {
    let (_s, c) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    let v1 = c.put_object("drv", "a.txt", "one", Default::default()).await.unwrap();
    let r = c.rename("drv", "a.txt", "moved/b é.txt", Default::default()).await.unwrap();
    assert_eq!(r.etag, v1.etag, "content and ETag stay");
    assert!(c.head_object("drv", "a.txt", Default::default()).await.unwrap_err().is_not_found());
    assert_eq!(c.list_versions("drv", "moved/b é.txt", false).await.unwrap().len(), 1, "history follows the object");
    assert_eq!(c.list_versions("drv", "moved/b é.txt", true).await.unwrap().last().unwrap().operation, "rename");

    // The atomic save: a temporary file renamed over the original.
    c.put_object("drv", "moved/.tmp", "two", Default::default()).await.unwrap();
    let e = c.rename("drv", "moved/.tmp", "moved/b é.txt", Default::default()).await.unwrap_err();
    assert_eq!(e.code(), Some("PathConflict"));
    c.rename("drv", "moved/.tmp", "moved/b é.txt", RenameOptions { replace: true, ..Default::default() }).await.unwrap();
    assert_eq!(text(&c, "drv", "moved/b é.txt").await, "two");

    c.rename("drv", "moved/", "kept/", Default::default()).await.unwrap();
    assert_eq!(text(&c, "drv", "kept/b é.txt").await, "two");
    let e = c.rename("drv", "kept/", "elsewhere/", RenameOptions { if_version: Some("999.0".into()), ..Default::default() }).await.unwrap_err();
    assert_eq!(e.status(), Some(412));
}

#[tokio::test(flavor = "multi_thread")]
async fn history_is_read_and_restored() {
    let (_s, c) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    let v1 = c.put_object("drv", "docs/a", "first", Default::default()).await.unwrap();
    let v2 = c.put_object("drv", "docs/a", "second", Default::default()).await.unwrap();
    c.put_object("drv", "docs/b", "b", Default::default()).await.unwrap();
    let history = c.list_versions("drv", "docs/a", false).await.unwrap();
    assert_eq!(history.iter().map(|v| v.version_id.as_str()).collect::<Vec<_>>(), [v1.version_id.as_str(), v2.version_id.as_str()]);
    assert_eq!((history[0].is_latest, history[1].is_latest, history[0].operation.as_str()), (false, true, "put"));

    let old = c.get_object("drv", "docs/a", ReadOptions { version_id: Some(v1.version_id.clone()), ..Default::default() }).await.unwrap();
    assert_eq!((&old.body[..], old.meta.version_id.as_str()), (&b"first"[..], v1.version_id.as_str()));
    let then = c.get_object("drv", "docs/a", ReadOptions { as_of: Some(history[0].last_modified.clone()), ..Default::default() }).await.unwrap();
    assert_eq!(&then.body[..], b"first", "lastModified passes unchanged to as_of");
    let both = ReadOptions { version_id: Some(v1.version_id.clone()), as_of: Some(history[0].last_modified.clone()) };
    assert!(matches!(c.get_object("drv", "docs/a", both).await, Err(Error::Invalid(_))));
    assert_eq!(c.get_object("drv", "docs/a", ReadOptions { version_id: Some("9999.0".into()), ..Default::default() }).await.unwrap_err().code(), Some("NoSuchVersion"));

    let r = c.restore_version("drv", "docs/a", &v1.version_id, Default::default()).await.unwrap();
    assert_eq!(r.restored_from.as_deref(), Some(v1.version_id.as_str()));
    assert_ne!(r.version_id, v1.version_id, "a new version");
    assert_eq!(text(&c, "drv", "docs/a").await, "first");
    let h = c.list_versions("drv", "docs/a", false).await.unwrap();
    assert_eq!((h.len(), h[2].operation.as_str(), h[2].restored_from.as_deref()), (3, "restore", Some(v1.version_id.as_str())));

    // A folder rolled back to an instant: changed files restored, new ones removed.
    let at = c.list_versions("drv", "docs/b", false).await.unwrap()[0].last_modified.clone();
    c.put_object("drv", "docs/a", "third", Default::default()).await.unwrap();
    c.put_object("drv", "docs/new", "new", Default::default()).await.unwrap();
    c.restore_as_of("drv", "docs/", &at, Default::default()).await.unwrap();
    assert_eq!(text(&c, "drv", "docs/a").await, "second");
    assert!(c.head_object("drv", "docs/new", Default::default()).await.unwrap_err().is_not_found());
    // A file, as of an instant.
    let r = c.restore_as_of("drv", "docs/a", &history[0].last_modified, Default::default()).await.unwrap();
    assert_eq!(r.restored_from.as_deref(), Some(v1.version_id.as_str()));
    assert_eq!(text(&c, "drv", "docs/a").await, "first");
}

#[tokio::test(flavor = "multi_thread")]
async fn histories_and_listings_follow_every_page() {
    let (_s, c) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    let mut last = None;
    for i in 0..1003 {
        last = Some(c.put_object("drv", "many", format!("{i}"), Default::default()).await.unwrap().version_id);
    }
    let h = c.list_versions("drv", "many", false).await.unwrap();
    assert_eq!(h.len(), 1003);
    assert_eq!(h.last().map(|v| &v.version_id), last.as_ref());
    assert_eq!(h.iter().filter(|v| v.is_latest).count(), 1);

    let mut puts = Vec::new();
    for i in 0..1003 {
        let c = c.clone();
        puts.push(tokio::spawn(async move { c.put_object("drv", &format!("dir/f{i:04}"), "x", Default::default()).await.unwrap() }));
    }
    for p in puts {
        p.await.unwrap();
    }
    let l = c.list_folder("drv", "dir/").await.unwrap();
    assert_eq!(l.entries.len(), 1003);
    assert_eq!(l.entries[1002].name, "f1002");
    let page = c.list_folder_page("drv", "dir/", None).await.unwrap();
    assert_eq!((page.entries.len(), page.next_continuation_token.as_deref()), (1000, Some("f0999")));
    let all = c.list_objects("drv", ListOptions { prefix: Some("dir/".into()), ..Default::default() }).await.unwrap();
    assert_eq!(all.objects.len(), 1003);
}

#[tokio::test(flavor = "multi_thread")]
async fn attributes_listings_and_deleted_objects() {
    let (_s, c) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    c.put_object("drv", "top/f", "12345", Default::default()).await.unwrap();
    c.put_object("drv", "top/sub/", Bytes::new(), Default::default()).await.unwrap();
    let update = AttributesUpdate {
        mtime: Some("2026-01-02T03:04:05.000006Z".into()),
        mode: Some(0o600),
        set_xattrs: BTreeMap::from([("user.tag".to_string(), Bytes::from_static(b"\x00red"))]),
        flags: Some(vec!["hidden".into()]),
        content_type: Some("text/x-test".into()),
        ..Default::default()
    };
    let w = c.set_attributes("drv", "top/f", update, Default::default()).await.unwrap();
    let a = c.attributes("drv", "top/f", Default::default()).await.unwrap();
    assert_eq!((a.kind, a.mode.as_deref(), a.mtime.as_deref()), (Kind::File, Some("0600"), Some("2026-01-02T03:04:05.000006Z")));
    assert_eq!(a.xattrs.get("user.tag").map(String::as_str), Some("AHJlZA=="));
    assert_eq!((a.flags, a.content_type.as_deref(), a.version_id), (vec!["hidden".to_string()], Some("text/x-test"), Some(w.version_id.clone())));
    c.set_attributes("drv", "top/f", AttributesUpdate { remove_xattrs: vec!["user.tag".into()], ..Default::default() }, Preconditions::if_version(w.version_id)).await.unwrap();
    assert!(c.attributes("drv", "top/f", Default::default()).await.unwrap().xattrs.is_empty());

    let l = c.list_folder("drv", "top/").await.unwrap();
    assert_eq!(l.prefix, "top/");
    assert_eq!(l.seq, c.describe_drive("drv").await.unwrap().seq);
    let names: Vec<_> = l.entries.iter().map(|e| (e.name.as_str(), e.kind, e.size)).collect();
    assert_eq!(names, [("f", Kind::File, Some(5)), ("sub/", Kind::Folder, None)]);
    let root = c.list_folder("drv", "").await.unwrap();
    assert_eq!(root.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["top/"]);
    assert_eq!(c.list_folder("drv", "top").await.unwrap_err().code(), Some("InvalidArgument"));

    let listed = c.list_objects("drv", ListOptions { prefix: Some("top/".into()), delimiter: Some("/".into()) }).await.unwrap();
    assert_eq!(listed.objects.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(), ["top/f"]);
    assert_eq!(listed.common_prefixes, ["top/sub/"]);

    let before = c.head_object("drv", "top/f", Default::default()).await.unwrap();
    c.delete_object("drv", "top/f", Default::default()).await.unwrap();
    let deleted = c.list_deleted("drv", "top/").await.unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!((deleted[0].key.as_str(), deleted[0].last_version_id.as_str(), deleted[0].size), ("top/f", before.version_id.as_str(), 5));
    c.restore_version("drv", "top/f", &deleted[0].last_version_id, Default::default()).await.unwrap();
    assert_eq!(text(&c, "drv", "top/f").await, "12345");
    assert!(c.list_deleted("drv", "").await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_change_feed_is_polled_and_watched() {
    let (_s, c) = setup().await;
    c.create_drive("drv", Default::default()).await.unwrap();
    let start = c.describe_drive("drv").await.unwrap().seq;
    let nothing = c.changes("drv", start, Duration::ZERO).await.unwrap();
    assert_eq!((nothing.seq, nothing.changes.len(), nothing.more), (start, 0, false));

    let writer = c.clone();
    let put = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        writer.put_object("drv", "a/b", "x", Default::default()).await.unwrap()
    });
    let got = c.changes("drv", start, Duration::from_secs(10)).await.unwrap();
    let v = put.await.unwrap();
    assert_eq!(got.changes.iter().map(|ch| (ch.op.as_str(), ch.key.as_str())).collect::<Vec<_>>(), [("create", "a/"), ("put", "a/b")], "the folder first");
    assert_eq!(got.changes[1].version_id, v.version_id);
    assert_eq!(got.changes[1].seq, Some(got.seq));

    let mut watch = c.watch_changes("drv", got.seq);
    c.rename("drv", "a/b", "a/c", Default::default()).await.unwrap();
    c.delete_object("drv", "a/c", Default::default()).await.unwrap();
    let b1 = watch.next().await.unwrap();
    assert_eq!((b1.changes[0].op.as_str(), b1.changes[0].key.as_str(), b1.changes[0].from_key.as_deref()), ("rename", "a/c", Some("a/b")));
    let b2 = watch.next().await.unwrap();
    assert_eq!((b2.changes[0].op.as_str(), b2.changes[0].key.as_str()), ("delete", "a/c"));
    assert!(b2.seq > b1.seq);
    assert_eq!(watch.position(), b2.seq);

    let mut old = c.watch_changes("nowhere", 0);
    assert!(old.next().await.unwrap_err().is_not_found());
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_carry_the_status_code_and_request() {
    let read = KeyInfo { id: "VFREADKEY23456723456".into(), secret: "readsecretreadsecretreadsecretreadsecret".into(), scope: Scope::Read, drives: None };
    let s = TestServer::with_keys(vec![read.clone()]).await.unwrap();
    let admin = common::client_for(&s.endpoint, Config::default());
    admin.create_drive("drv", Default::default()).await.unwrap();
    admin.put_object("drv", "k", "v", Default::default()).await.unwrap();
    let reader = Client::new(Config { endpoint: s.endpoint.clone(), access_key_id: read.id.clone(), secret_access_key: read.secret.clone(), ..Default::default() }).unwrap();
    assert_eq!(text(&reader, "drv", "k").await, "v");
    let e = reader.put_object("drv", "k", "w", Default::default()).await.unwrap_err();
    assert_eq!((e.status(), e.code()), (Some(403), Some("AccessDenied")));
    assert!(e.request_id().is_some());
    let e = reader.list_drives().await.map(|d| d.len()).unwrap();
    assert_eq!(e, 1);

    let wrong = Client::new(Config { endpoint: s.endpoint.clone(), access_key_id: read.id.clone(), secret_access_key: "not the secret".into(), ..Default::default() }).unwrap();
    assert_eq!(wrong.get_object("drv", "k", Default::default()).await.unwrap_err().code(), Some("SignatureDoesNotMatch"));
    assert_eq!(wrong.list_drives().await.unwrap_err().code(), Some("SignatureDoesNotMatch"), "the AWS client's errors too");

    let e = admin.storage_credentials("drv").await.unwrap_err();
    assert_eq!((e.status(), e.code()), (Some(501), Some("NotImplemented")));
    assert_eq!(admin.describe_drive("nowhere").await.unwrap_err().code(), Some("NoSuchBucket"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_aws_client_underneath_shares_the_drive() {
    let s = common::server().await;
    // A host name, not an address: the AWS client would address a name virtual-host style, which
    // this server isn't set up for, unless told to use paths.
    let c = common::client_for(&format!("http://localhost:{}", s.addr.port()), Config::default());
    c.create_drive("drv", Default::default()).await.unwrap();
    let s3 = c.s3();
    let up = s3.create_multipart_upload().bucket("drv").key("mp").send().await.unwrap();
    let id = up.upload_id().unwrap();
    let part = vec![7u8; 5 << 20];
    let p1 = s3.upload_part().bucket("drv").key("mp").upload_id(id).part_number(1).body(part.clone().into()).send().await.unwrap();
    let p2 = s3.upload_part().bucket("drv").key("mp").upload_id(id).part_number(2).body(b"tail".to_vec().into()).send().await.unwrap();
    use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
    let parts = CompletedMultipartUpload::builder()
        .parts(CompletedPart::builder().part_number(1).e_tag(p1.e_tag().unwrap()).build())
        .parts(CompletedPart::builder().part_number(2).e_tag(p2.e_tag().unwrap()).build())
        .build();
    s3.complete_multipart_upload().bucket("drv").key("mp").upload_id(id).multipart_upload(parts).send().await.unwrap();
    let o = c.get_object("drv", "mp", Default::default()).await.unwrap();
    assert_eq!(o.meta.size, (5 << 20) + 4);
    assert_eq!(&o.body[5 << 20..], b"tail");
    assert_eq!(c.list_versions("drv", "mp", false).await.unwrap().len(), 1, "one version");
    // Its errors are this crate's.
    let missing = async { Ok::<_, Error>(s3.head_object().bucket("drv").key("missing").send().await?) };
    let e = missing.await.unwrap_err();
    assert_eq!((e.status(), e.code()), (Some(404), Some("NotFound")));
    let e: Error = s3.get_object().bucket("drv").key("missing").send().await.unwrap_err().into();
    assert_eq!((e.status(), e.code()), (Some(404), Some("NoSuchKey")));
}
