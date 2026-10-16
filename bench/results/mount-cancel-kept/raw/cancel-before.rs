// The test that showed what cancelling a mount's change did before the kept fate, run at
// main (c8e37a1) by appending this to crates/voidfs-client/tests/mount_publication.rs.
// Its output is cancel-before.txt.

fn stage_paths(f: &Fixture) -> Vec<String> {
    f.db().prepare("SELECT path FROM mount_staged").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

fn sources(f: &Fixture) -> Vec<String> {
    f.db().prepare("SELECT source FROM entries WHERE source IS NOT NULL").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

async fn read_all(ns: &Session, ino: u64) -> Bytes {
    let fh = ns.open(ino, false).await.unwrap();
    let bytes = ns.read(fh, 0, 1024).await.unwrap();
    ns.close(fh).await.unwrap();
    bytes
}

/// What cancelling a mounted file's unpublished change does today, before its fate is decided
/// (step 5, item 8): this Mac keeps the cancelled bytes as an error, and nothing about the file
/// publishes again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_mount_change_today_keeps_its_bytes_here_and_strands_the_file() {
    let f = Fixture::new(Backend::Api).await;
    let ns = f.session().await;
    let (ino, fh) = existing(&f, &ns).await;
    ns.write(fh, 0, Bytes::from_static(b"LOCAL")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    let queued = f.queue.status().await.unwrap().items;
    assert_eq!(queued.iter().map(|i| (i.op, i.state)).collect::<Vec<_>>(), [(voidfs_client::Op::Write, State::Queued), (voidfs_client::Op::Truncate, State::Queued)]);
    let (frozen, stage) = (sources(&f), stage_paths(&f));
    assert_eq!((frozen.len(), stage.len()), (1, 1));
    f.queue.cancel(Scope::Entry(queued[0].id)).await.unwrap();
    eprintln!("cancelled: {:?}", f.queue.status().await.unwrap().items.iter().map(|i| (i.id, i.op, i.state)).collect::<Vec<_>>());
    assert!(f.queue.status().await.unwrap().items.iter().all(|i| i.state == State::Cancelled), "the change's truncate is cancelled with it");
    assert!(!Path::new(&frozen[0]).exists(), "the queue's frozen copy of the change is deleted");
    assert_eq!(stage_paths(&f), stage, "the mount's own staged file stays");
    assert!(Path::new(&stage[0]).exists());
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Error);
    assert_eq!(ns.read(fh, 0, 1024).await.unwrap(), Bytes::from_static(b"LOCAL56789ab"), "an open handle keeps the cancelled bytes");
    ns.close(fh).await.unwrap();
    assert_eq!(read_all(&ns, ino).await, Bytes::from_static(b"LOCAL56789ab"), "so does a new open: the copy here isn't the drive's");
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"0123456789ab")));
    f.publish().await;
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"0123456789ab")), "a cancelled change never publishes");
    eprintln!("this Mac {:?} ({:?}), the drive {:?}", read_all(&ns, ino).await, ns.getattr(ino).await.unwrap().sync, f.body("file").await);

    // A later save of the same file is built on the cancelled change, and fails for good.
    let fh = ns.open(ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"AGAIN")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    ns.close(fh).await.unwrap();
    f.publish().await;
    let later = f.queue.status().await.unwrap().items.into_iter().filter(|i| i.state != State::Cancelled).collect::<Vec<_>>();
    eprintln!("a later save: {:?}", later.iter().map(|i| (i.id, i.op, i.state, i.error.clone())).collect::<Vec<_>>());
    assert_eq!(later[0].state, State::Failed);
    assert_eq!(later[0].error.as_deref(), Some("the mount edit's predecessor was not published"));
    assert!(later[1..].iter().all(|i| i.state == State::Queued), "what follows it waits behind it");
    f.queue.resume(Scope::All).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), f.queue.settle()).await.unwrap();
    assert_eq!(f.queue.status().await.unwrap().items.iter().find(|i| i.id == later[0].id).unwrap().state, State::Failed, "retrying fails the same way");
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"0123456789ab")));
    assert_eq!(read_all(&ns, ino).await, Bytes::from_static(b"AGAIN56789ab"));
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Error);

    // A restart changes nothing.
    let (f, ns) = f.restart(ns).await;
    assert_eq!(read_all(&ns, ino).await, Bytes::from_static(b"AGAIN56789ab"));
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Error);

    // Another computer's save doesn't show here, and isn't a conflict.
    f.put("file", b"from another mac").await;
    ns.invalidate(&[Invalidation::All]).await.unwrap();
    assert_eq!(read_all(&ns, ino).await, Bytes::from_static(b"AGAIN56789ab"));
    assert_eq!(ns.getattr(ino).await.unwrap().sync, Sync::Error);

    // Deleting it here goes from this Mac and never from the drive.
    ns.unlink(ns.root(), "file").await.unwrap();
    f.publish().await;
    let deletion = f.queue.status().await.unwrap().items.into_iter().find(|i| i.op == voidfs_client::Op::Delete).unwrap();
    eprintln!("the deletion: {:?} {:?}", deletion.state, deletion.error);
    assert_eq!(deletion.state, State::Queued);
    assert!(ns.readdir(ns.root(), None, 10).await.unwrap().is_empty());
    assert_eq!(f.body("file").await, Some(Bytes::from_static(b"from another mac")));

    // A new file's cancelled creation: here as an error, never on the drive.
    let new = ns.create(ns.root(), "new", 0o600).await.unwrap();
    let fh = ns.open(new.ino, true).await.unwrap();
    ns.write(fh, 0, Bytes::from_static(b"fresh")).await.unwrap();
    ns.fsync(fh).await.unwrap();
    ns.close(fh).await.unwrap();
    let id = f.queue.status().await.unwrap().items.iter().find(|i| i.key == "new" && !i.state.finished()).unwrap().id;
    f.queue.cancel(Scope::Entry(id)).await.unwrap();
    f.publish().await;
    assert_eq!(read_all(&ns, new.ino).await, Bytes::from_static(b"fresh"));
    assert_eq!(ns.getattr(new.ino).await.unwrap().sync, Sync::Error);
    assert_eq!(f.body("new").await, None);
    f.queue.close().await;
}
