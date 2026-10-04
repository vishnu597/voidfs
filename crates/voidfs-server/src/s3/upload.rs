// SPDX-License-Identifier: Apache-2.0
//! Direct uploads (protocol §4.11). A plan says which shards the pool holds and presigns a PUT
//! to the bucket for each of the rest; a commit checks its token, confirms that every shard it
//! does not already reference is in the bucket with its length, and commits a put of the shards
//! through the ordinary path. The store's binding of each URL to the shard's checksum vouches for
//! the bytes (protocol §9).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;
use voidfs_core::ids::{ShardHash, VersionId};
use voidfs_core::model::{ContentDescriptor, Extent, Op};
use voidfs_core::ops;

use super::object::{mutation_response, parse_key};
use super::util::{BodyReader, Ctx, MAX_SMALL_BODY, json};
use super::{App, S3Error};
use crate::direct::{self, Direct, MAX_SHARD, MAX_SHARDS, PLAN_TTL};
use crate::pool::{CommitError, Drive, Found};

#[derive(Deserialize)]
struct Listed {
    hash: String,
    length: u64,
}

#[derive(Deserialize)]
struct PlanBody {
    shards: Vec<Listed>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitBody {
    token: String,
    size: u64,
    content_sha256: String,
    shards: Vec<Listed>,
}

fn offered(app: &App) -> Result<&Direct, S3Error> {
    app.direct.as_deref().ok_or_else(|| S3Error::not_implemented("direct uploads are not offered here: the store cannot presign uploads that bind a shard's checksum. Send an ordinary PUT"))
}

fn body_of<T: for<'de> Deserialize<'de>>(bytes: &[u8], what: &str) -> Result<T, S3Error> {
    serde_json::from_slice(bytes).map_err(|e| S3Error::invalid(format!("the {what}'s body is not what protocol §4.11 describes: {e}")))
}

fn shards(listed: Vec<Listed>) -> Result<Vec<(ShardHash, u64)>, S3Error> {
    if listed.len() > MAX_SHARDS {
        return Err(S3Error::invalid(format!("a plan lists at most {MAX_SHARDS} shards, not {}", listed.len())));
    }
    listed
        .into_iter()
        .map(|s| {
            let h = s.hash.parse::<ShardHash>().map_err(|_| S3Error::invalid(format!("{:?} is not a shard's hash: 64 lowercase hexadecimal digits", s.hash)))?;
            if s.length == 0 || s.length > MAX_SHARD {
                return Err(S3Error::invalid(format!("shard {h} is listed with {} bytes; a shard has 1 to {MAX_SHARD}", s.length)));
            }
            Ok((h, s.length))
        })
        .collect()
}

/// Each shard once, in the order first listed.
fn distinct(list: &[(ShardHash, u64)]) -> Vec<(ShardHash, u64)> {
    let mut seen = std::collections::HashSet::new();
    list.iter().filter(|(h, _)| seen.insert(*h)).copied().collect()
}

/// The key's head, and the shards its content references with their lengths: the pool holds
/// them for as long as that version is the head (format §12.4, option 1).
async fn referenced(app: &App, d: &Drive, key: &str) -> Result<(Option<VersionId>, HashMap<ShardHash, u64>), S3Error> {
    let snap = d.snapshot();
    let Some(record) = parse_key(key).ok().and_then(|k| snap.lookup(&k)).and_then(|o| snap.record(&o)) else { return Ok((None, HashMap::new())) };
    let mut known = HashMap::new();
    if let Some(desc) = &record.content {
        for e in app.pool.extents(desc).await? {
            if let Extent::Shard { s, n } = e {
                known.insert(s, n);
            }
        }
    }
    Ok((Some(record.head), known))
}

/// The listed shards that `known` lists too, after checking their lengths agree.
fn split(list: &[(ShardHash, u64)], known: &HashMap<ShardHash, u64>) -> Result<(usize, Vec<(ShardHash, u64)>), S3Error> {
    let mut rest = Vec::new();
    let mut held = 0;
    for (h, n) in list {
        match known.get(h) {
            Some(k) if k == n => held += 1,
            Some(k) => return Err(S3Error::invalid(format!("shard {h} is {k} bytes, not {n}"))),
            None => rest.push((*h, *n)),
        }
    }
    Ok((held, rest))
}

pub async fn plan(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>, body: Body) -> Result<Response, S3Error> {
    let direct = offered(app)?;
    parse_key(ctx.key())?;
    let req: PlanBody = body_of(&BodyReader::new(body, ctx, MAX_SMALL_BODY)?.read_all().await?, "plan")?;
    let list = shards(req.shards)?;
    let unique = distinct(&list);
    let (_, known) = referenced(app, d, ctx.key()).await?;
    let (mut held, rest) = split(&unique, &known)?;
    let found = app.pool.find_shards(&rest, false).await?;
    let mut binds = Vec::new();
    let mut upload = Vec::new();
    for ((h, n), f) in rest.iter().zip(found) {
        match f {
            Found::Held => held += 1,
            Found::Length(len) => return Err(S3Error::invalid(format!("shard {h} is {len} bytes, not {n}"))),
            Found::Missing => {
                binds.clear();
                binds.push(("x-amz-checksum-sha256".to_owned(), direct::checksum(h)));
                if direct.if_none_match {
                    binds.push(("if-none-match".to_owned(), "*".to_owned()));
                }
                let p = direct.presign.put(&format!("shards/{}", h.object_path()), &binds, PLAN_TTL).await?;
                let headers: serde_json::Map<String, serde_json::Value> = p.headers.into_iter().map(|(k, v)| (k, v.into())).collect();
                upload.push(json!({ "hash": h.to_hex(), "length": n, "url": p.url, "headers": headers }));
            }
        }
    }
    let expires = app.pool.clock.now().datetime().timestamp() + PLAN_TTL.as_secs() as i64;
    let token = direct.token(&d.id, ctx.key(), &list, expires);
    Ok(json(&json!({ "token": token, "expiresSeconds": PLAN_TTL.as_secs(), "held": held, "upload": upload })))
}

pub async fn commit(app: &Arc<App>, ctx: &Ctx, d: &Arc<Drive>, body: Body) -> Result<Response, S3Error> {
    let direct = offered(app)?;
    let req: CommitBody = body_of(&BodyReader::new(body, ctx, MAX_SMALL_BODY)?.read_all().await?, "commit")?;
    let list = shards(req.shards)?;
    if req.content_sha256.len() != 64 || !req.content_sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(S3Error::invalid("contentSha256 must be 64 lowercase hexadecimal digits"));
    }
    let size: u64 = list.iter().map(|(_, n)| n).sum();
    if size != req.size {
        return Err(S3Error::invalid(format!("the shards cover {size} bytes, not the size given, {}", req.size)));
    }
    let now = app.pool.clock.now().datetime().timestamp();
    direct.check(&req.token, &d.id, ctx.key(), &list, now).map_err(|e| S3Error::invalid(e.to_string()))?;
    let attrs = ctx.attrs_for_commit()?;
    let pre = ctx.precondition();
    let actor = ctx.actor();
    let unique = distinct(&list);
    for _ in 0..8 {
        let (base_head, known) = referenced(app, d, ctx.key()).await?;
        let (relied, rest) = split(&unique, &known)?;
        let found = app.pool.find_shards(&rest, true).await.map_err(|e| S3Error::new(503, "SlowDown", format!("{e:#}")))?;
        for ((h, n), f) in rest.iter().zip(found) {
            match f {
                Found::Held => {}
                Found::Missing => return Err(S3Error::invalid(format!("shard {h} is not in the bucket: PUT it to the URL its plan gave, or plan again"))),
                Found::Length(len) => return Err(S3Error::invalid(format!("shard {h} is in the bucket with {len} bytes, not {n}"))),
            }
        }
        let desc = match list.is_empty() {
            true => ContentDescriptor::empty(),
            false => app.pool.describe(list.iter().map(|(h, n)| Extent::Shard { s: *h, n: *n }).collect()).await?,
        };
        let (key, attrs, pre, actor) = (ctx.key().to_owned(), attrs.clone(), pre.clone(), actor.clone());
        let r = app
            .pool
            .commit(d, move |st| {
                // Shards taken as held because the head references them are held only while it does.
                if relied > 0 && parse_key(&key).ok().and_then(|k| st.lookup(&k)).and_then(|o| st.record(&o)).map(|r| r.head) != base_head {
                    return Err(CommitError::Retry);
                }
                Ok(ops::put(st, &key, desc.clone(), attrs.clone(), Op::Put, &pre, &actor)?)
            })
            .await;
        match r {
            Ok((v, s)) => return Ok(mutation_response(v, &s, ctx.key()).body(Body::empty()).unwrap()),
            Err(CommitError::Retry) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(CommitError::Retry.into())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use bytes::Bytes;
    use futures::FutureExt;
    use http::Method;
    use opendal::Buffer;
    use opendal::raw::HttpClient;
    use rand::{RngExt, SeedableRng};
    use serde_json::Value;
    use voidfs_core::ids::Timestamp;
    use voidfs_core::model::{Chunking, CommitGuard, Features, PoolDescriptor};

    use super::*;
    use crate::clock::Clock;
    use crate::gc::{PENDING, PendingRecord, Phase};
    use crate::pool::Pool;
    use crate::s3::util::Query;
    use crate::sigv4::{Authenticated, KeyInfo, Payload, Scope};
    use crate::store::{Fault, MemOp, MemStore, Store};
    use crate::test_server::{FakeBucket, Rules};

    struct Setup {
        app: Arc<App>,
        mem: Arc<MemStore>,
        bucket: FakeBucket,
        http: HttpClient,
    }

    fn random_bytes(seed: u64, len: usize) -> Vec<u8> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        (0..len).map(|_| rng.random()).collect()
    }

    /// A pool over `mem` that cuts shards of 64 bytes to 1 KiB, with drive `d`, and direct
    /// uploads to a [`FakeBucket`] that binds what its URLs carry.
    async fn setup_on(mem: Arc<MemStore>, clock: Clock) -> Setup {
        let store = Store::mem(mem.clone());
        if store.get(crate::probe::DESCRIPTOR).await.unwrap().is_none() {
            let desc = PoolDescriptor {
                format: voidfs_core::FORMAT_VERSION,
                pool_id: "p-direct".into(),
                created: Timestamp::now(),
                features: Features { compatible: vec![], incompatible: vec![] },
                chunking: Chunking { min: 64, avg: 256, max: 1024, ..Chunking::default() },
                hash: "sha256".into(),
                commit_guard: CommitGuard::CreateIfAbsent,
            };
            store.put(crate::probe::DESCRIPTOR, Bytes::from(serde_json::to_vec(&desc).unwrap())).await.unwrap();
        }
        let pool = Pool::open_with(store.clone(), 1 << 20, clock).await.unwrap();
        if pool.drive("d").is_none() {
            pool.create_drive("d", None).await.unwrap();
        }
        let bucket = FakeBucket::start(store).await.unwrap();
        let http = HttpClient::new().unwrap();
        let (_, direct) = crate::probe::offer(Box::new(bucket.clone()), &http).await;
        assert!(direct.is_some(), "the fake bucket binds checksums");
        let app = Arc::new(App { pool, keys: crate::sigv4::Keys::default(), domains: super::super::Domains::new(Vec::new()), metrics: crate::metrics::S3Metrics::new(), uploads: Default::default(), read_ahead: Default::default(), direct });
        Setup { app, mem, bucket, http }
    }

    async fn setup() -> Setup {
        setup_on(Arc::new(MemStore::new(Clock::System)), Clock::System).await
    }

    async fn call(app: &Arc<App>, method: Method, key: &str, query: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Result<Response, S3Error> {
        let auth = Authenticated {
            key: KeyInfo { id: "k".into(), secret: "s".into(), scope: Scope::Admin, drives: None },
            payload: Payload::Unsigned,
            signing_key: [0; 32],
            scope: String::new(),
            amz_date: String::new(),
            seed_signature: String::new(),
        };
        let mut map = http::HeaderMap::new();
        for (k, v) in headers {
            map.insert(http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        let ctx = Ctx { method, bucket: Some("d".into()), key: Some(key.into()), virtual_host: false, query: Query::parse(query), headers: map, auth };
        super::super::object::dispatch(app, &ctx, Body::from(body)).await
    }

    async fn body_of(r: Response) -> Bytes {
        axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()
    }

    /// The content cut as the pool cuts it.
    fn cut(s: &Setup, data: &[u8]) -> Vec<voidfs_core::chunk::Shard> {
        voidfs_core::chunk::shards(&Bytes::copy_from_slice(data), s.app.pool.params)
    }

    fn listed(shards: &[(ShardHash, u64)]) -> Value {
        Value::Array(shards.iter().map(|(h, n)| json!({ "hash": h.to_hex(), "length": n })).collect())
    }

    fn list_of(s: &Setup, data: &[u8]) -> Vec<(ShardHash, u64)> {
        cut(s, data).iter().map(|x| (x.hash, x.bytes.len() as u64)).collect()
    }

    async fn plan_list(s: &Setup, key: &str, list: &[(ShardHash, u64)]) -> Result<Value, S3Error> {
        let r = call(&s.app, Method::POST, key, "x-voidfs-upload-plan", &[], serde_json::to_vec(&json!({ "shards": listed(list) })).unwrap()).await?;
        Ok(serde_json::from_slice(&body_of(r).await).unwrap())
    }

    async fn plan(s: &Setup, key: &str, data: &[u8]) -> Value {
        plan_list(s, key, &list_of(s, data)).await.unwrap_or_else(|e| panic!("{} {}", e.code, e.message))
    }

    async fn put_url(s: &Setup, url: &str, headers: &serde_json::Map<String, Value>, body: Bytes) -> u16 {
        let mut req = http::Request::put(url);
        for (k, v) in headers {
            req = req.header(k, v.as_str().unwrap());
        }
        s.http.send(req.body(Buffer::from(body)).unwrap()).await.unwrap().status().as_u16()
    }

    /// PUTs the shards the plan lists for upload but those whose hashes `skip` names.
    async fn upload(s: &Setup, plan: &Value, data: &[u8], skip: &[ShardHash]) {
        let shards = cut(s, data);
        for u in plan["upload"].as_array().unwrap() {
            let h: ShardHash = u["hash"].as_str().unwrap().parse().unwrap();
            if skip.contains(&h) {
                continue;
            }
            let bytes = shards.iter().find(|x| x.hash == h).unwrap().bytes.clone();
            assert_eq!(put_url(s, u["url"].as_str().unwrap(), u["headers"].as_object().unwrap(), bytes).await, 200);
        }
    }

    fn commit_body(list: &[(ShardHash, u64)], token: &str, data: &[u8]) -> Vec<u8> {
        let size: u64 = list.iter().map(|(_, n)| n).sum();
        serde_json::to_vec(&json!({ "token": token, "size": size, "contentSha256": hex::encode(Sha256Hash::digest(data)), "shards": listed(list) })).unwrap()
    }

    use sha2::{Digest as _, Sha256 as Sha256Hash};

    async fn commit(s: &Setup, key: &str, list: &[(ShardHash, u64)], token: &str, data: &[u8], headers: &[(&str, &str)]) -> Result<Response, S3Error> {
        commit_to(&s.app, key, list, token, data, headers).await
    }

    async fn commit_to(app: &Arc<App>, key: &str, list: &[(ShardHash, u64)], token: &str, data: &[u8], headers: &[(&str, &str)]) -> Result<Response, S3Error> {
        call(app, Method::PUT, key, "x-voidfs-upload-commit", headers, commit_body(list, token, data)).await
    }

    async fn read(s: &Setup, key: &str) -> Bytes {
        body_of(call(&s.app, Method::GET, key, "", &[], Vec::new()).await.unwrap()).await
    }

    fn token(p: &Value) -> String {
        p["token"].as_str().unwrap().to_owned()
    }

    fn uploads(p: &Value) -> Vec<ShardHash> {
        p["upload"].as_array().unwrap().iter().map(|u| u["hash"].as_str().unwrap().parse().unwrap()).collect()
    }

    /// Counts the HEADs of shards from now on.
    fn heads(mem: &MemStore) -> Arc<AtomicUsize> {
        let n = Arc::new(AtomicUsize::new(0));
        let seen = n.clone();
        mem.set_hook(Some(Arc::new(move |op, path| {
            if op == MemOp::Head && path.starts_with("shards/") {
                seen.fetch_add(1, Ordering::SeqCst);
            }
            futures::future::ready(Fault::None).boxed()
        })));
        n
    }

    /// A plan holds what the key's version references, without asking the bucket; what the
    /// bucket has, by a HEAD, or by this server's recent checks; and presigns the rest, binding
    /// each shard's checksum and create-if-absent.
    #[tokio::test]
    async fn a_plan_says_what_the_pool_holds_and_presigns_the_rest() {
        let s = setup().await;
        let data = random_bytes(1, 20_000);
        call(&s.app, Method::PUT, "k", "", &[], data.clone()).await.unwrap();
        let n = list_of(&s, &data).len();
        assert!(n > 10);
        let counted = heads(&s.mem);
        let p = plan(&s, "k", &data).await;
        assert_eq!((p["held"].as_u64(), uploads(&p).len()), (Some(n as u64), 0));
        assert_eq!(counted.load(Ordering::SeqCst), 0, "the head's shards need no request");
        assert_eq!(p["expiresSeconds"], 900);

        // One region changed: the shards around it are new.
        let mut edited = data.clone();
        edited[10_000..10_010].copy_from_slice(b"0123456789");
        let p = plan(&s, "k", &edited).await;
        let new: Vec<_> = list_of(&s, &edited).into_iter().filter(|x| !list_of(&s, &data).contains(x)).collect();
        assert!(!new.is_empty() && new.len() < 4, "{new:?}");
        assert_eq!(uploads(&p), new.iter().map(|x| x.0).collect::<Vec<_>>());
        assert_eq!(p["held"].as_u64(), Some((list_of(&s, &edited).len() - new.len()) as u64));
        let u = &p["upload"][0];
        assert_eq!(u["length"], new[0].1);
        assert_eq!(u["headers"]["x-amz-checksum-sha256"], crate::direct::checksum(&new[0].0));
        assert_eq!(u["headers"]["if-none-match"], "*");
        assert!(u["url"].as_str().unwrap().starts_with(&format!("{}/shards/{}", s.bucket.endpoint, new[0].0.object_path())));

        assert_eq!(counted.load(Ordering::SeqCst), new.len(), "a HEAD for each new shard");

        // At another key, the shards this server uploaded count as checked.
        counted.store(0, Ordering::SeqCst);
        let p = plan(&s, "other", &data).await;
        assert_eq!((p["held"].as_u64(), counted.load(Ordering::SeqCst)), (Some(n as u64), 0));

        // A server that has checked nothing asks the bucket, a HEAD each.
        let s2 = setup_on(s.mem.clone(), Clock::System).await;
        let counted = heads(&s2.mem);
        let p = plan(&s2, "other", &data).await;
        assert_eq!(p["held"].as_u64(), Some(n as u64));
        assert_eq!(counted.load(Ordering::SeqCst), n);
        let p = plan(&s2, "other", &random_bytes(2, 3000)).await;
        assert_eq!(p["held"], 0);
    }

    /// The shards a plan lists, PUT to its URLs, and a commit with its token: a version like a
    /// put's, with the commit's content type, metadata and modification time.
    #[tokio::test]
    async fn a_commit_puts_the_shards_it_planned() {
        let s = setup().await;
        let data = random_bytes(3, 30_000);
        let list = list_of(&s, &data);
        let p = plan(&s, "f", &data).await;
        assert_eq!(uploads(&p).len(), list.len());
        upload(&s, &p, &data, &[]).await;
        let headers = [("x-voidfs-content-type", "video/quicktime"), ("x-amz-meta-voidfs-entry", "e1"), ("x-voidfs-mtime", "2026-01-02T03:04:05Z")];
        let r = commit(&s, "f", &list, &token(&p), &data, &headers).await.unwrap();
        assert_eq!(r.status(), 200);
        let version = r.headers()["x-amz-version-id"].to_str().unwrap().to_owned();
        assert_eq!(r.headers()["x-voidfs-size"], "30000");
        let etag = r.headers()["etag"].clone();
        assert_eq!(read(&s, "f").await, data);
        let h = call(&s.app, Method::HEAD, "f", "", &[], Vec::new()).await.unwrap();
        assert_eq!(h.headers()["content-type"], "video/quicktime");
        assert_eq!(h.headers()["x-amz-meta-voidfs-entry"], "e1");
        assert_eq!(h.headers()["x-voidfs-mtime"], "2026-01-02T03:04:05.000000Z");
        assert_eq!(h.headers()["x-amz-version-id"].to_str().unwrap(), version);
        // The same bytes put the ordinary way are the same content.
        let put = call(&s.app, Method::PUT, "g", "", &[], data.clone()).await.unwrap();
        assert_eq!(put.headers()["etag"], etag);
        // An empty object needs no shards.
        let p = plan(&s, "empty", b"").await;
        let r = commit(&s, "empty", &[], &token(&p), b"", &[]).await.unwrap();
        assert_eq!(r.headers()["x-voidfs-size"], "0");
        assert_eq!(read(&s, "empty").await, Bytes::new());
    }

    /// Shards that repeat are listed for upload once, and the content keeps every repeat.
    #[tokio::test]
    async fn repeated_shards_are_uploaded_once() {
        let s = setup().await;
        let piece = random_bytes(4, 600);
        let list = [(ShardHash::of(&piece), 600u64); 3];
        let p = plan_list(&s, "r", &list).await.unwrap();
        assert_eq!(uploads(&p).len(), 1);
        let data = piece.repeat(3);
        let u = &p["upload"][0];
        assert_eq!(put_url(&s, u["url"].as_str().unwrap(), u["headers"].as_object().unwrap(), Bytes::from(piece.clone())).await, 200);
        commit(&s, "r", &list, &token(&p), &data, &[]).await.unwrap();
        assert_eq!(read(&s, "r").await, data);
    }

    fn refused<T>(r: Result<T, S3Error>, status: u16, says: &str) -> S3Error {
        let e = r.err().unwrap_or_else(|| panic!("expected {status}"));
        assert_eq!(e.status.as_u16(), status, "{}", e.message);
        assert!(e.message.contains(says), "{:?} does not say {says:?}", e.message);
        e
    }

    /// A commit needs every shard it does not reference in the bucket, with its length; nothing
    /// is committed otherwise.
    #[tokio::test]
    async fn a_commit_is_refused_for_a_shard_not_in_the_bucket() {
        let s = setup().await;
        let data = random_bytes(5, 8_000);
        let list = list_of(&s, &data);
        let p = plan(&s, "m", &data).await;
        upload(&s, &p, &data, &[list[2].0]).await;
        refused(commit(&s, "m", &list, &token(&p), &data, &[]).await, 400, &format!("shard {} is not in the bucket", list[2].0));
        assert_eq!(call(&s.app, Method::HEAD, "m", "", &[], Vec::new()).await.err().unwrap().status, 404);
        // Uploaded later, under the same plan, it commits.
        upload(&s, &p, &data, &list.iter().map(|x| x.0).filter(|h| *h != list[2].0).collect::<Vec<_>>()).await;
        commit(&s, "m", &list, &token(&p), &data, &[]).await.unwrap();
        assert_eq!(read(&s, "m").await, data);
    }

    /// A length that disagrees with the shard's is refused: one the key's head references, and
    /// one in the bucket.
    #[tokio::test]
    async fn a_shard_listed_with_the_wrong_length_is_refused() {
        let s = setup().await;
        let data = random_bytes(6, 4_000);
        call(&s.app, Method::PUT, "k", "", &[], data.clone()).await.unwrap();
        let mut list = list_of(&s, &data);
        list[0].1 += 1;
        refused(plan_list(&s, "k", &list).await, 400, &format!("shard {} is {} bytes, not {}", list[0].0, list[0].1 - 1, list[0].1));
        let s2 = setup_on(s.mem.clone(), Clock::System).await;
        refused(plan_list(&s2, "elsewhere", &list).await, 400, &format!("shard {} is {} bytes, not {}", list[0].0, list[0].1 - 1, list[0].1));
    }

    /// The token holds only for the drive, key and shard list of its plan, and for 900 seconds.
    #[tokio::test]
    async fn a_commit_needs_the_token_of_its_plan() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let clock = Clock::manual(Timestamp::now());
        let s = setup_on(mem, clock.clone()).await;
        let data = random_bytes(7, 5_000);
        let list = list_of(&s, &data);
        let p = plan(&s, "t", &data).await;
        upload(&s, &p, &data, &[]).await;
        let t = token(&p);
        let (expiry, mac) = t.split_once('.').unwrap();
        let forged = format!("{expiry}.{}{}", if mac.starts_with('A') { "B" } else { "A" }, &mac[1..]);
        refused(commit(&s, "t", &list, &forged, &data, &[]).await, 400, "not issued by this server");
        refused(commit(&s, "u", &list, &t, &data, &[]).await, 400, "not issued by this server");
        let reversed: Vec<_> = list.iter().rev().copied().collect();
        refused(commit(&s, "t", &reversed, &t, &data, &[]).await, 400, "not issued by this server");
        refused(commit(&s, "t", &list[1..], &t, &data, &[]).await, 400, "not issued by this server");
        let later: i64 = expiry.parse::<i64>().unwrap() + 3600;
        refused(commit(&s, "t", &list, &format!("{later}.{mac}"), &data, &[]).await, 400, "not issued by this server");
        refused(commit(&s, "t", &list, "nonsense", &data, &[]).await, 400, "malformed");
        // The size must be the shards'.
        let mut body: Value = serde_json::from_slice(&commit_body(&list, &t, &data)).unwrap();
        body["size"] = json!(1);
        refused(call(&s.app, Method::PUT, "t", "x-voidfs-upload-commit", &[], serde_json::to_vec(&body).unwrap()).await, 400, "the shards cover");
        body["size"] = json!(data.len());
        body["contentSha256"] = json!("XYZ");
        refused(call(&s.app, Method::PUT, "t", "x-voidfs-upload-commit", &[], serde_json::to_vec(&body).unwrap()).await, 400, "contentSha256");
        clock.advance(Duration::from_secs(900));
        refused(commit(&s, "t", &list, &t, &data, &[]).await, 400, "expired");
        let p = plan(&s, "t", &data).await;
        commit(&s, "t", &list, &token(&p), &data, &[]).await.unwrap();
    }

    /// A commit's preconditions are a put's: the version, the ETag, or none at all.
    #[tokio::test]
    async fn a_commit_keeps_the_preconditions_of_a_put() {
        let s = setup().await;
        let data = random_bytes(8, 3_000);
        let first = call(&s.app, Method::PUT, "c", "", &[], b"before".to_vec()).await.unwrap();
        let version = first.headers()["x-amz-version-id"].to_str().unwrap().to_owned();
        let list = list_of(&s, &data);
        let p = plan(&s, "c", &data).await;
        upload(&s, &p, &data, &[]).await;
        let e = refused(commit(&s, "c", &list, &token(&p), &data, &[("x-voidfs-if-version", "99.0")]).await, 412, "");
        assert_eq!(e.headers.iter().find(|(k, _)| *k == "x-amz-version-id").map(|(_, v)| v.as_str()), Some(version.as_str()));
        refused(commit(&s, "c", &list, &token(&p), &data, &[("if-none-match", "*")]).await, 412, "");
        refused(commit(&s, "c", &list, &token(&p), &data, &[("if-match", "\"nope\"")]).await, 412, "");
        assert_eq!(read(&s, "c").await, &b"before"[..]);
        commit(&s, "c", &list, &token(&p), &data, &[("x-voidfs-if-version", &version)]).await.unwrap();
        assert_eq!(read(&s, "c").await, data);
        // A folder in the way is a conflict.
        call(&s.app, Method::PUT, "dir/x", "", &[], b"x".to_vec()).await.unwrap();
        let p = plan(&s, "dir", &data).await;
        refused(commit(&s, "dir", &list, &token(&p), &data, &[]).await, 409, "");
    }

    /// Plans that break the limits are refused before anything is asked of the bucket.
    #[tokio::test]
    async fn plans_outside_the_limits_are_refused() {
        let s = setup().await;
        let h = ShardHash::of(b"x");
        refused(plan_list(&s, "l", &vec![(h, 1); MAX_SHARDS + 1]).await, 400, "at most 4096");
        plan_list(&s, "l", &vec![(h, 1); MAX_SHARDS]).await.unwrap();
        refused(plan_list(&s, "l", &[(h, 0)]).await, 400, "1 to 16777216");
        refused(plan_list(&s, "l", &[(h, MAX_SHARD + 1)]).await, 400, "1 to 16777216");
        let bad = call(&s.app, Method::POST, "l", "x-voidfs-upload-plan", &[], br#"{"shards":[{"hash":"ABC","length":1}]}"#.to_vec()).await;
        refused(bad, 400, "not a shard's hash");
        refused(call(&s.app, Method::POST, "l", "x-voidfs-upload-plan", &[], b"{}".to_vec()).await, 400, "protocol");
    }

    /// Without a store that binds checksums, the extension is not offered.
    #[tokio::test]
    async fn without_presigning_plans_and_commits_answer_501() {
        let s = setup().await;
        let app = Arc::new(App { pool: s.app.pool.clone(), keys: crate::sigv4::Keys::default(), domains: super::super::Domains::new(Vec::new()), metrics: crate::metrics::S3Metrics::new(), uploads: Default::default(), read_ahead: Default::default(), direct: None });
        let list = list_of(&s, b"abc");
        refused(call(&app, Method::POST, "x", "x-voidfs-upload-plan", &[], serde_json::to_vec(&json!({ "shards": listed(&list) })).unwrap()).await, 501, "not offered");
        refused(call(&app, Method::PUT, "x", "x-voidfs-upload-commit", &[], commit_body(&list, "1.x", b"abc")).await, 501, "not offered");
    }

    /// What a server finds of a store's presigned PUTs decides whether it offers direct uploads,
    /// and whether its URLs bind create-if-absent. The checks store one valid shard.
    #[tokio::test]
    async fn presigned_puts_say_what_the_store_binds() {
        let mem = Arc::new(MemStore::new(Clock::System));
        let store = Store::mem(mem.clone());
        let bucket = FakeBucket::start(store.clone()).await.unwrap();
        let http = HttpClient::new().unwrap();
        let check = async |rules: Rules| {
            bucket.set_rules(rules);
            let (checks, direct) = crate::probe::offer(Box::new(bucket.clone()), &http).await;
            (checks, direct.map(|d| d.if_none_match))
        };
        let (c, d) = check(Rules::default()).await;
        assert_eq!((c.checksum.clone(), c.signed_header.clone(), c.if_none_match.clone(), d), (crate::probe::Check::Enforced, crate::probe::Check::Enforced, crate::probe::Check::Enforced, Some(true)));
        assert!(c.to_string().ends_with("Direct uploads are offered, binding both"), "{c}");
        let (c, d) = check(Rules { if_none_match: None, ..Rules::default() }).await;
        assert_eq!((c.if_none_match.clone(), d), (crate::probe::Check::Unsupported, Some(false)));
        let (c, d) = check(Rules { if_none_match: Some(false), ..Rules::default() }).await;
        assert_eq!((c.if_none_match.clone(), d), (crate::probe::Check::Ignored, Some(false)));
        let (c, d) = check(Rules { checksum: false, ..Rules::default() }).await;
        assert_eq!((c.checksum.clone(), d), (crate::probe::Check::Ignored, None));
        assert!(c.to_string().contains("NOT offered"), "{c}");
        let (c, d) = check(Rules { signed_headers: false, ..Rules::default() }).await;
        assert_eq!((c.signed_header.clone(), d), (crate::probe::Check::Ignored, None));
        let all = store.list_recursive("").await.unwrap();
        assert_eq!(all.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), [format!("shards/{}", ShardHash::of(crate::probe::PROBE_SHARD).object_path())]);
        assert_eq!(store.get(&all[0].name).await.unwrap().unwrap(), crate::probe::PROBE_SHARD);
    }

    async fn run(s: &Setup, phase: Phase, candidates: Vec<ShardHash>) {
        let rec = PendingRecord { format: 1, run: "r1".into(), phase, t1: Some(Timestamp::now()), grace: 86_400, candidates };
        s.app.pool.store.put(PENDING, Bytes::from(serde_json::to_vec(&rec).unwrap())).await.unwrap();
        s.app.pool.guard.refresh(&s.app.pool.store, &s.app.pool.clock).await.unwrap();
    }

    /// A shard proposed for deletion is held only once it is rewritten (format §12.4, option
    /// 3): a plan does that from the bucket's copy. One a run is deleting is not held, and a
    /// commit that needs it fails, to be tried again.
    #[tokio::test]
    async fn shards_proposed_for_deletion_are_rescued_or_waited_for() {
        let s = setup().await;
        let data = random_bytes(9, 3_000);
        let list = list_of(&s, &data);
        let p = plan(&s, "a", &data).await;
        upload(&s, &p, &data, &[]).await;
        let s = setup_on(s.mem.clone(), Clock::System).await;
        run(&s, Phase::Waiting, vec![list[0].0]).await;
        let rewrites = Arc::new(AtomicUsize::new(0));
        let seen = rewrites.clone();
        let target = format!("shards/{}", list[0].0.object_path());
        s.mem.set_hook(Some(Arc::new(move |op, path| {
            if op == MemOp::Put && path == target {
                seen.fetch_add(1, Ordering::SeqCst);
            }
            futures::future::ready(Fault::None).boxed()
        })));
        let p = plan(&s, "a", &data).await;
        assert_eq!((p["held"].as_u64(), rewrites.load(Ordering::SeqCst)), (Some(list.len() as u64), 1));
        commit(&s, "a", &list, &token(&p), &data, &[]).await.unwrap();
        assert_eq!(read(&s, "a").await, data);

        let s = setup_on(s.mem.clone(), Clock::System).await;
        run(&s, Phase::Deleting, vec![list[1].0]).await;
        let p = plan(&s, "b", &data).await;
        assert_eq!(uploads(&p), [list[1].0]);
        let e = refused(commit(&s, "b", &list, &token(&p), &data, &[]).await, 503, "garbage collection is deleting");
        assert_eq!(e.code, "SlowDown");
    }

    /// Shards held because the key's head references them count only while it is the head: a
    /// commit that finds another head plans its checks again.
    #[tokio::test]
    async fn a_commit_checks_again_when_the_head_moves() {
        let s = setup().await;
        let data = random_bytes(10, 6_000);
        call(&s.app, Method::PUT, "k", "", &[], data.clone()).await.unwrap();
        let mut edited = data.clone();
        edited[3000..3004].copy_from_slice(b"edit");
        let list = list_of(&s, &edited);
        let p = plan(&s, "k", &edited).await;
        let new = uploads(&p);
        assert!(!new.is_empty() && new.len() < list.len());
        upload(&s, &p, &edited, &[]).await;
        // While the commit asks the bucket about the new shards, the key changes.
        let gate = Arc::new(tokio::sync::Notify::new());
        let asked = Arc::new(AtomicUsize::new(0));
        let (counted, release) = (asked.clone(), gate.clone());
        s.mem.set_hook(Some(Arc::new(move |op, path| {
            let release = release.clone();
            let first = op == MemOp::Head && path.starts_with("shards/") && counted.fetch_add(1, Ordering::SeqCst) == 0;
            async move {
                if first {
                    release.notified().await;
                }
                Fault::None
            }
            .boxed()
        })));
        let committing = tokio::spawn({
            let (app, list, t, edited) = (s.app.clone(), list.clone(), token(&p), edited.clone());
            async move { commit_to(&app, "k", &list, &t, &edited, &[]).await }
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            while asked.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        call(&s.app, Method::PUT, "k", "", &[], b"meanwhile".to_vec()).await.unwrap();
        gate.notify_one();
        let r = tokio::time::timeout(Duration::from_secs(10), committing).await.unwrap().unwrap().unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(read(&s, "k").await, edited);
        let distinct: std::collections::HashSet<_> = list.iter().map(|x| x.0).collect();
        assert_eq!(asked.load(Ordering::SeqCst), new.len() + distinct.len(), "the first try asked about the new shards, the second about every one");
    }
}
