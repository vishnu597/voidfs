// SPDX-License-Identifier: Apache-2.0
//! Short-lived storage credentials (protocol §5.5): read-only credentials to a drive's storage,
//! so that a mount or a bulk reader fetches shards and metadata straight from the bucket and the
//! server carries no content bytes.
//!
//! They reach what a reader of the drive needs (format §2): the pool's descriptor, the shared
//! `shards/` and `pages/`, and the drive's own prefix, which they may also list ([`readable`]).
//! [`Sts`] mints them through the bucket's STS with AssumeRole and a session policy that narrows
//! the deployment's credentials to those paths: MinIO's, and AWS's with a role to assume. A server
//! offers them only where a check at start ([`check`]) finds minted credentials that read those
//! paths and are refused the pool's root, another drive and a write; anything else answers `501`,
//! and clients read through the API.
//!
//! Revoking an access key does not reach credentials already issued (protocol §5.5): they last
//! [`TTL`], the shortest AWS and MinIO allow. A drive's are minted once and shared by every key
//! that reads it, since they reach the same paths, until less than [`RENEW_WITHIN`] is left.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use opendal::Buffer;
use opendal::raw::HttpClient;
use reqsign_aws_v4::{Credential, RequestSigner, StaticCredentialProvider};
use reqsign_core::{ProvideCredentialChain, Signer};
use sha2::{Digest, Sha256};
use voidfs_core::ids::{DriveId, ShardHash};

use crate::sigv4::Keys;

/// How long minted credentials last...
pub const TTL: Duration = Duration::from_secs(900);
/// ...and how much of that is left, at least, in every answer: a drive's are minted again once
/// less is.
pub const RENEW_WITHIN: Duration = Duration::from_secs(450);

/// What a key segment of a URL keeps as it is: unreserved characters and `/`.
const KEY: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~').remove(b'/');
/// What a form or query value keeps as it is: unreserved characters.
const VALUE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// What credentials for `drive` may read, relative to the pool's root (format §2): the pool's
/// descriptor, which a reader checks first (format §3.1), the shared shards and pages, and the
/// drive's own prefix, which they may also list. An entry ending in `/` is a prefix.
pub fn readable(drive: &DriveId) -> Vec<String> {
    vec![voidfs_format::DESCRIPTOR.to_owned(), "shards/".into(), "pages/".into(), format!("drives/{drive}/")]
}

/// The drive's access generation (protocol §5.5): a digest of the access rules that reach it,
/// each key's id and scope, so that it changes whenever they do. Keys are fixed while a server
/// runs, so it changes only when a restart changes them. 48 bits, which a JSON number holds
/// exactly in every language.
pub fn access_generation(keys: &Keys, alias: &str, id: &str) -> u64 {
    let mut rules: Vec<String> = keys.iter().filter(|k| k.reaches(alias, id)).map(|k| format!("{} {:?}", k.id, k.scope)).collect();
    rules.sort();
    let h = Sha256::digest(rules.join("\n"));
    u64::from_be_bytes([0, 0, h[0], h[1], h[2], h[3], h[4], h[5]])
}

/// Credentials a [`Mint`] made.
#[derive(Clone)]
pub struct Minted {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub expires_at: DateTime<Utc>,
}

impl fmt::Debug for Minted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Minted").field("access_key_id", &self.access_key_id).field("expires_at", &self.expires_at).finish_non_exhaustive()
    }
}

/// Something that mints read-only credentials scoped to a drive.
pub trait Mint: Send + Sync {
    /// Credentials that read what [`readable`] lists for `drive` and list the drive's prefix, and
    /// nothing else, lasting `ttl`.
    fn mint<'a>(&'a self, drive: &'a DriveId, ttl: Duration) -> BoxFuture<'a, anyhow::Result<Minted>>;
}

/// Where credentials reach: the bucket, as protocol §5.5's `storage` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub bucket: String,
    /// The pool's root in the bucket: empty, or ending in `/`.
    pub root: String,
    pub region: String,
    /// The endpoint the server reaches the bucket at, which it addresses path-style.
    pub endpoint: String,
}

/// Storage credentials as a server offers them: minted per drive, and shared until they are due
/// again.
pub struct Credentials {
    pub location: Location,
    mint: Box<dyn Mint>,
    drives: Mutex<HashMap<DriveId, Arc<tokio::sync::Mutex<Option<Minted>>>>>,
}

impl Credentials {
    pub fn new(location: Location, mint: Box<dyn Mint>) -> Credentials {
        Credentials { location, mint, drives: Mutex::default() }
    }

    /// Credentials for `drive`: those minted last, if more than [`RENEW_WITHIN`] of them is left,
    /// or new ones. One mint at a time per drive.
    pub async fn for_drive(&self, drive: &DriveId) -> anyhow::Result<Minted> {
        let slot = self.drives.lock().unwrap().entry(drive.clone()).or_default().clone();
        let mut held = slot.lock().await;
        if let Some(m) = held.as_ref()
            && (m.expires_at - Utc::now()).to_std().is_ok_and(|left| left > RENEW_WITHIN)
        {
            return Ok(m.clone());
        }
        let m = self.mint.mint(drive, TTL).await?;
        *held = Some(m.clone());
        Ok(m)
    }
}

// ---------------------------------------------------------------------------------------------
// STS

/// What a server needs to mint credentials through STS, from its flags.
#[derive(Clone, Debug, Default)]
pub struct StsOptions {
    /// The role to assume, which AWS needs.
    pub role: Option<String>,
    /// The STS endpoint, if not the default for the bucket.
    pub endpoint: Option<String>,
}

/// The session policy that narrows the deployment's credentials to what a reader of `drive`
/// needs, for a pool at `root` in `bucket`.
pub fn session_policy(bucket: &str, root: &str, drive: &DriveId) -> String {
    let objects: Vec<String> = readable(drive).iter().map(|p| format!("arn:aws:s3:::{bucket}/{root}{p}{}", if p.ends_with('/') { "*" } else { "" })).collect();
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": ["s3:GetObject"], "Resource": objects },
            { "Effect": "Allow", "Action": ["s3:ListBucket"], "Resource": [format!("arn:aws:s3:::{bucket}")], "Condition": { "StringLike": { "s3:prefix": [format!("{root}drives/{drive}/*")] } } },
        ],
    })
    .to_string()
}

/// Mints with STS AssumeRole and a [`session_policy`], signed with the deployment's own
/// credentials for the bucket: MinIO's STS on the bucket's endpoint, which needs no role, or AWS's,
/// which needs one that allows reading the pool and trusts those credentials.
pub struct Sts {
    pub http: HttpClient,
    pub signer: Signer<Credential>,
    /// The STS endpoint, as a URL.
    pub endpoint: String,
    pub role: Option<String>,
    pub bucket: String,
    pub root: String,
}

impl Mint for Sts {
    fn mint<'a>(&'a self, drive: &'a DriveId, ttl: Duration) -> BoxFuture<'a, anyhow::Result<Minted>> {
        Box::pin(async move {
            let mut form = vec![
                ("Action", "AssumeRole".to_owned()),
                ("Version", "2011-06-15".to_owned()),
                ("DurationSeconds", ttl.as_secs().to_string()),
                ("RoleSessionName", format!("voidfs-{drive}")),
                ("Policy", session_policy(&self.bucket, &self.root, drive)),
            ];
            if let Some(role) = &self.role {
                form.push(("RoleArn", role.clone()));
            }
            let body = form.iter().map(|(k, v)| format!("{k}={}", percent_encoding::utf8_percent_encode(v, VALUE))).collect::<Vec<_>>().join("&");
            let (mut parts, ()) = http::Request::post(&self.endpoint)
                .header(http::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("x-amz-content-sha256", hex::encode(Sha256::digest(&body)))
                .body(())?
                .into_parts();
            self.signer.sign(&mut parts, None).await.map_err(|e| anyhow!("signing AssumeRole: {e}"))?;
            let resp = self.http.send(http::Request::from_parts(parts, Buffer::from(Bytes::from(body)))).await.context("calling STS AssumeRole")?;
            let status = resp.status().as_u16();
            let text = String::from_utf8_lossy(&resp.into_body().to_bytes()).into_owned();
            if status != 200 {
                bail!("STS AssumeRole answered HTTP {status}{}", sts_error(&text).map(|e| format!(": {e}")).unwrap_or_default());
            }
            parse_assume_role(&text)
        })
    }
}

/// The `Code` and `Message` of an STS error document.
fn sts_error(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let err = doc.descendants().find(|n| n.has_tag_name("Error"))?;
    let text = |name: &str| err.children().find(|c| c.has_tag_name(name)).and_then(|c| c.text()).map(str::trim).unwrap_or_default().to_owned();
    Some(format!("{} {}", text("Code"), text("Message")).trim().trim_end_matches('.').to_owned())
}

/// The credentials in an AssumeRole answer.
fn parse_assume_role(xml: &str) -> anyhow::Result<Minted> {
    let doc = roxmltree::Document::parse(xml).context("parsing the AssumeRole answer")?;
    let c = doc.descendants().find(|n| n.has_tag_name("Credentials")).ok_or_else(|| anyhow!("the AssumeRole answer has no credentials"))?;
    let text = |name: &str| c.children().find(|n| n.has_tag_name(name)).and_then(|n| n.text()).map(|t| t.trim().to_owned()).ok_or_else(|| anyhow!("the AssumeRole answer has no {name}"));
    let expires = text("Expiration")?;
    Ok(Minted {
        access_key_id: text("AccessKeyId")?,
        secret_access_key: text("SecretAccessKey")?,
        session_token: text("SessionToken").ok(),
        expires_at: DateTime::parse_from_rfc3339(&expires).with_context(|| format!("the AssumeRole answer's expiration {expires:?}"))?.with_timezone(&Utc),
    })
}

// ---------------------------------------------------------------------------------------------
// The check at start

/// The drive the check mints for, and another, which no pool has.
const CHECK_DRIVE: &str = "d-00000000-0000-4000-8000-000000000000";
const OTHER_DRIVE: &str = "d-00000000-0000-4000-8000-000000000001";

/// What minted credentials did, checked when a server starts.
#[derive(Clone, Debug)]
pub struct Checks {
    /// Why none could be minted, if none could.
    pub refused: Option<String>,
    /// What was tried, whether it went as it must, and what happened.
    pub tried: Vec<(&'static str, bool, String)>,
}

impl Checks {
    /// Whether the credentials read what a reader of a drive needs and nothing else, so that a
    /// server may offer them.
    pub fn scoped(&self) -> bool {
        self.refused.is_none() && !self.tried.is_empty() && self.tried.iter().all(|t| t.1)
    }
}

impl fmt::Display for Checks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(why) = &self.refused {
            return write!(f, "none could be minted: {why}. Storage credentials are NOT offered");
        }
        let said: Vec<String> = self.tried.iter().map(|(what, held, got)| format!("{what}: {}{got}", if *held { "" } else { "UNEXPECTED, " })).collect();
        write!(f, "{}. ", said.join("; "))?;
        if self.scoped() {
            write!(f, "Storage credentials are offered, for {} minutes", TTL.as_secs() / 60)
        } else {
            write!(f, "Storage credentials are NOT offered: minted credentials must read only what a drive's reader needs")
        }
    }
}

/// Mints credentials for a drive no pool has and tries them on the bucket at `location`: they
/// must read `voidfs.json` and list their drive's prefix, and be refused a listing of the pool's
/// root, another drive's prefix, and a write (a PUT of [`crate::probe::PROBE_SHARD`], a valid
/// shard, so that a bucket that took it stores nothing a pool must not hold).
pub async fn check(mint: &dyn Mint, location: &Location, http: &HttpClient) -> Checks {
    let drive: DriveId = CHECK_DRIVE.parse().expect("a drive id");
    let m = match mint.mint(&drive, TTL).await {
        Ok(m) => m,
        Err(e) => return Checks { refused: Some(format!("{e:#}")), tried: Vec::new() },
    };
    let mut provider = StaticCredentialProvider::new(&m.access_key_id, &m.secret_access_key);
    if let Some(t) = &m.session_token {
        provider = provider.with_session_token(t);
    }
    let ctx = reqsign_core::Context::new();
    let signer = Signer::new(ctx, ProvideCredentialChain::new().push(provider), RequestSigner::new("s3", &location.region));
    let base = format!("{}/{}", location.endpoint.trim_end_matches('/'), location.bucket);
    let send = async |method: http::Method, url: String, body: Bytes| -> Result<u16, String> {
        let (mut parts, ()) = http::Request::builder().method(method).uri(url).header("x-amz-content-sha256", hex::encode(Sha256::digest(&body))).body(()).map_err(|e| e.to_string())?.into_parts();
        signer.sign(&mut parts, None).await.map_err(|e| format!("signing: {e}"))?;
        let resp = http.send(http::Request::from_parts(parts, Buffer::from(body))).await.map_err(|e| e.to_string())?;
        Ok(resp.status().as_u16())
    };
    let object = |path: &str| format!("{base}/{}", percent_encoding::utf8_percent_encode(&format!("{}{path}", location.root), KEY));
    let listing = |prefix: &str| format!("{base}?delimiter=%2F&list-type=2&max-keys=1&prefix={}", percent_encoding::utf8_percent_encode(&format!("{}{prefix}", location.root), VALUE));
    let shard = ShardHash::of(crate::probe::PROBE_SHARD);
    let attempts = [
        ("reading voidfs.json", http::Method::GET, object(voidfs_format::DESCRIPTOR), Bytes::new(), true),
        ("listing its drive", http::Method::GET, listing(&format!("drives/{drive}/")), Bytes::new(), true),
        ("listing the pool's root", http::Method::GET, listing(""), Bytes::new(), false),
        ("listing another drive", http::Method::GET, listing(&format!("drives/{OTHER_DRIVE}/")), Bytes::new(), false),
        ("reading another drive", http::Method::GET, object(&format!("drives/{OTHER_DRIVE}/drive.json")), Bytes::new(), false),
        ("a write", http::Method::PUT, object(&voidfs_format::shard_path(&shard)), Bytes::from_static(crate::probe::PROBE_SHARD), false),
    ];
    let mut tried = Vec::new();
    for (what, method, url, body, allowed) in attempts {
        let (held, got) = match send(method, url, body).await {
            // MinIO refuses some requests with 400 AccessDenied (item 5).
            Ok(s) if allowed => (s == 200, format!("HTTP {s}")),
            Ok(s @ (400 | 403)) => (true, format!("refused (HTTP {s})")),
            Ok(s) => (false, format!("HTTP {s}")),
            Err(e) => (false, e),
        };
        tried.push((what, held, got));
    }
    Checks { refused: None, tried }
}

/// Storage credentials through `mint`, if the credentials it mints are scoped as they must be:
/// what a server offers, after the check it makes when it starts.
pub async fn offer(mint: Box<dyn Mint>, location: Location, http: &HttpClient) -> (Checks, Option<Arc<Credentials>>) {
    let checks = check(mint.as_ref(), &location, http).await;
    let offered = checks.scoped().then(|| Arc::new(Credentials::new(location, mint)));
    (checks, offered)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::sigv4::{KeyInfo, Scope};
    use crate::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, FAKE_BUCKET, FAKE_ROOT, Rules, TestServer};

    const READ_KEY: &str = "VFTESTREADKEY2345678";
    const READ_SECRET: &str = "readsecretreadsecretreadsecretreadsecret";

    /// A request signed with `id`, `secret` and `token`: its status, headers and body.
    async fn signed(method: http::Method, url: &str, (id, secret, token): (&str, &str, Option<&str>), body: &'static [u8]) -> (u16, http::HeaderMap, Bytes) {
        let mut provider = StaticCredentialProvider::new(id, secret);
        if let Some(t) = token {
            provider = provider.with_session_token(t);
        }
        let signer = Signer::new(reqsign_core::Context::new(), ProvideCredentialChain::new().push(provider), RequestSigner::new("s3", "us-east-1"));
        let (mut parts, ()) = http::Request::builder().method(method).uri(url).header("x-amz-content-sha256", hex::encode(Sha256::digest(body))).body(()).unwrap().into_parts();
        signer.sign(&mut parts, None).await.unwrap();
        let resp = HttpClient::new().unwrap().send(http::Request::from_parts(parts, Buffer::from(Bytes::from_static(body)))).await.unwrap();
        let (parts, body) = resp.into_parts();
        (parts.status.as_u16(), parts.headers, body.to_bytes())
    }

    fn read_key(drives: &[&str]) -> KeyInfo {
        KeyInfo { id: READ_KEY.into(), secret: READ_SECRET.into(), scope: Scope::Read, drives: Some(drives.iter().map(|d| d.to_string()).collect()) }
    }

    /// A server with storage credentials under `rules`, a read key for `drv`, and drives `drv`
    /// and `other` with a file each.
    async fn setup(rules: Rules) -> TestServer {
        let s = TestServer::with_storage_credentials(vec![read_key(&["drv"])], rules).await.unwrap();
        for d in ["drv", "other"] {
            let admin = (ADMIN_KEY_ID, ADMIN_SECRET, None);
            assert_eq!(signed(http::Method::PUT, &format!("{}/{d}", s.endpoint), admin, b"").await.0, 200);
            assert_eq!(signed(http::Method::PUT, &format!("{}/{d}/f.bin", s.endpoint), admin, &[7; 5000]).await.0, 200);
        }
        s
    }

    async fn credentials_of(s: &TestServer, drive: &str) -> (u16, http::HeaderMap, Value) {
        let (status, headers, body) = signed(http::Method::GET, &format!("{}/{drive}?x-voidfs-credentials=", s.endpoint), (READ_KEY, READ_SECRET, None), b"").await;
        (status, headers, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    fn error_code(body: &Bytes) -> String {
        crate::probe::error_code(&String::from_utf8_lossy(body)).unwrap_or_default()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_drives_reader_gets_credentials_for_its_paths_only() {
        let s = setup(Rules::default()).await;
        let bucket = s.bucket.clone().unwrap();
        let drv = s.pool.drive("drv").unwrap();
        let other = s.pool.drive("other").unwrap();
        let (status, headers, c) = credentials_of(&s, "drv").await;
        assert_eq!(status, 200, "{c}");
        assert_eq!(headers.get(http::header::CACHE_CONTROL).unwrap(), "no-store");
        assert_eq!(c["driveId"], drv.id.as_str());
        let mut keys = Keys::default();
        keys.insert(KeyInfo { id: ADMIN_KEY_ID.into(), secret: ADMIN_SECRET.into(), scope: Scope::Admin, drives: None });
        keys.insert(read_key(&["drv"]));
        assert_eq!(c["accessGeneration"], access_generation(&keys, "drv", drv.id.as_str()));
        let st = &c["storage"];
        assert_eq!((st["backend"].as_str(), st["bucket"].as_str(), st["root"].as_str()), (Some("s3"), Some(FAKE_BUCKET), Some(FAKE_ROOT)));
        assert_eq!((st["endpoint"].as_str(), st["region"].as_str(), st["forcePathStyle"].as_bool()), (Some(bucket.endpoint.as_str()), Some("us-east-1"), Some(true)));
        assert_eq!(st["readable"], serde_json::json!(["voidfs.json", "shards/", "pages/", format!("drives/{}/", drv.id)]));
        let cr = &st["credentials"];
        let expires = DateTime::parse_from_rfc3339(cr["expiresAt"].as_str().unwrap()).unwrap().with_timezone(&Utc);
        let left = (expires - Utc::now()).num_seconds();
        assert!((TTL.as_secs() as i64 - 60..=TTL.as_secs() as i64).contains(&left), "{left} s left");
        assert!(c.get("storageBudget").is_none(), "no quotas yet");

        // What they reach: the pool's descriptor, shards and pages, and the drive's own prefix.
        let creds = (cr["accessKeyId"].as_str().unwrap(), cr["secretAccessKey"].as_str().unwrap(), cr["sessionToken"].as_str());
        let url = |path: &str| format!("{}/{FAKE_BUCKET}/{FAKE_ROOT}{path}", bucket.endpoint);
        let list = |prefix: &str| format!("{}/{FAKE_BUCKET}?delimiter=%2F&list-type=2&prefix={}", bucket.endpoint, percent_encoding::utf8_percent_encode(&format!("{FAKE_ROOT}{prefix}"), VALUE));
        let shard = s.pool.store.list_recursive("shards/").await.unwrap()[0].name.clone();
        for (what, method, u, want) in [
            ("the pool's descriptor", http::Method::GET, url("voidfs.json"), 200),
            ("the drive", http::Method::GET, url(&format!("drives/{}/drive.json", drv.id)), 200),
            ("its log", http::Method::GET, list(&format!("drives/{}/log/", drv.id)), 200),
            ("a shard", http::Method::GET, url(&format!("shards/{shard}")), 200),
            ("another drive", http::Method::GET, url(&format!("drives/{}/drive.json", other.id)), 403),
            ("another drive's log", http::Method::GET, list(&format!("drives/{}/log/", other.id)), 403),
            ("the pool's root", http::Method::GET, list(""), 403),
            ("a write", http::Method::PUT, url("shards/00/00/new"), 403),
        ] {
            let (status, _, body) = signed(method, &u, creds, b"").await;
            assert_eq!(status, want, "{what}: {}", error_code(&body));
        }
        // Without the session token, or with a changed secret, nothing.
        assert_eq!(signed(http::Method::GET, &url("voidfs.json"), (creds.0, creds.1, None), b"").await.0, 403);
        assert_eq!(signed(http::Method::GET, &url("voidfs.json"), (creds.0, "wrong", creds.2), b"").await.0, 403);

        // A drive the key doesn't reach doesn't exist for it, and neither does one nobody has.
        assert_eq!(credentials_of(&s, "other").await.0, 404);
        assert_eq!(credentials_of(&s, "nowhere").await.0, 404);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_drives_credentials_are_shared_until_they_are_due() {
        let s = setup(Rules::default()).await;
        let bucket = s.bucket.clone().unwrap();
        let at_start = bucket.credential_use().minted;
        assert_eq!(at_start, 1, "the check at start mints once");
        let (_, _, a) = credentials_of(&s, "drv").await;
        let (_, _, b) = credentials_of(&s, "drv").await;
        assert_eq!(a["storage"]["credentials"], b["storage"]["credentials"], "the same credentials");
        assert_eq!(bucket.credential_use().minted, at_start + 1);
        // Credentials with less than RENEW_WITHIN left are minted again.
        let s = setup(Rules { lifetime: Some(RENEW_WITHIN - Duration::from_secs(1)), ..Rules::default() }).await;
        let bucket = s.bucket.clone().unwrap();
        let (_, _, c) = credentials_of(&s, "drv").await;
        let (_, _, d) = credentials_of(&s, "drv").await;
        assert_ne!(c["storage"]["credentials"]["accessKeyId"], d["storage"]["credentials"]["accessKeyId"]);
        assert_eq!(bucket.credential_use().minted, 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn credentials_that_cant_be_minted_or_scoped_are_not_offered() {
        for (rules, why) in [(Rules { sts: false, ..Rules::default() }, "no STS"), (Rules { scope: false, ..Rules::default() }, "unscoped")] {
            let s = setup(rules).await;
            let (status, _, body) = signed(http::Method::GET, &format!("{}/drv?x-voidfs-credentials=", s.endpoint), (READ_KEY, READ_SECRET, None), b"").await;
            assert_eq!((status, error_code(&body).as_str()), (501, "NotImplemented"), "{why}");
        }
        let s = TestServer::with_keys(vec![read_key(&["drv"])]).await.unwrap();
        assert_eq!(signed(http::Method::PUT, &format!("{}/drv", s.endpoint), (ADMIN_KEY_ID, ADMIN_SECRET, None), b"").await.0, 200);
        assert_eq!(credentials_of(&s, "drv").await.0, 501, "a server without a bucket");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_mint_that_fails_later_answers_503() {
        let s = setup(Rules::default()).await;
        s.bucket.as_ref().unwrap().set_rules(Rules { sts: false, ..Rules::default() });
        let (status, _, body) = signed(http::Method::GET, &format!("{}/drv?x-voidfs-credentials=", s.endpoint), (READ_KEY, READ_SECRET, None), b"").await;
        assert_eq!((status, error_code(&body).as_str()), (503, "ServiceUnavailable"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_check_at_start_says_what_minted_credentials_reach() {
        let s = TestServer::with_direct_uploads(Vec::new()).await.unwrap();
        let bucket = s.bucket.clone().unwrap();
        let http = HttpClient::new().unwrap();
        let checks = check(&bucket, &bucket.location(), &http).await;
        assert!(checks.scoped(), "{checks}");
        assert_eq!(checks.tried.len(), 6);
        assert!(checks.to_string().ends_with("Storage credentials are offered, for 15 minutes"), "{checks}");
        bucket.set_rules(Rules { scope: false, ..Rules::default() });
        let checks = check(&bucket, &bucket.location(), &http).await;
        assert!(!checks.scoped());
        let wrong: Vec<&str> = checks.tried.iter().filter(|t| !t.1).map(|t| t.0).collect();
        assert_eq!(wrong, ["listing the pool's root", "listing another drive", "reading another drive", "a write"], "{checks}");
        assert!(checks.to_string().contains("NOT offered"), "{checks}");
        bucket.set_rules(Rules { sts: false, ..Rules::default() });
        let checks = check(&bucket, &bucket.location(), &http).await;
        assert!(!checks.scoped() && checks.refused.as_deref().unwrap().contains("501"), "{checks}");
    }

    /// An STS that answers AssumeRole with fixed credentials, and keeps what it was asked.
    async fn fake_sts(answer: &'static str, status: u16) -> (String, Arc<Mutex<Vec<(http::HeaderMap, String)>>>) {
        let asked: Arc<Mutex<Vec<(http::HeaderMap, String)>>> = Arc::default();
        let kept = asked.clone();
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let kept = kept.clone();
            async move {
                let (parts, body) = req.into_parts();
                let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
                kept.lock().unwrap().push((parts.headers, String::from_utf8(body.to_vec()).unwrap()));
                (http::StatusCode::from_u16(status).unwrap(), answer)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (endpoint, asked)
    }

    fn sts_at(endpoint: &str, role: Option<&str>) -> Sts {
        let b = crate::probe::Bucket::new("bkt", "pool", Some(endpoint), "us-east-1", Some(("AKEXAMPLE", "secretexample"))).unwrap();
        b.sts(&StsOptions { role: role.map(str::to_owned), endpoint: None }).unwrap()
    }

    #[tokio::test]
    async fn sts_is_asked_for_a_session_policy_and_its_answer_read() {
        let ok = "<AssumeRoleResponse><AssumeRoleResult><Credentials><AccessKeyId>ASIA1</AccessKeyId><SecretAccessKey>s1</SecretAccessKey><SessionToken>t1</SessionToken><Expiration>2030-01-01T00:15:00Z</Expiration></Credentials></AssumeRoleResult></AssumeRoleResponse>";
        let (endpoint, asked) = fake_sts(ok, 200).await;
        let d: DriveId = CHECK_DRIVE.parse().unwrap();
        let m = sts_at(&endpoint, Some("arn:aws:iam::123456789012:role/voidfs-read")).mint(&d, TTL).await.unwrap();
        assert_eq!((m.access_key_id.as_str(), m.session_token.as_deref()), ("ASIA1", Some("t1")));
        let (headers, body) = asked.lock().unwrap()[0].clone();
        let auth = headers.get(http::header::AUTHORIZATION).unwrap().to_str().unwrap();
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKEXAMPLE/") && auth.contains("/us-east-1/sts/aws4_request"), "{auth}");
        assert_eq!(headers.get("x-amz-content-sha256").unwrap().to_str().unwrap(), hex::encode(Sha256::digest(&body)), "the form is signed");
        let form: HashMap<String, String> = body.split('&').map(|p| p.split_once('=').unwrap()).map(|(k, v)| (k.to_owned(), percent_encoding::percent_decode_str(v).decode_utf8().unwrap().into_owned())).collect();
        assert_eq!((form["Action"].as_str(), form["Version"].as_str(), form["DurationSeconds"].as_str()), ("AssumeRole", "2011-06-15", "900"));
        assert_eq!(form["RoleArn"], "arn:aws:iam::123456789012:role/voidfs-read");
        assert_eq!(form["RoleSessionName"], format!("voidfs-{d}"));
        assert_eq!(form["Policy"], session_policy("bkt", "pool/", &d));
        // Without a role, as MinIO takes it, none is sent.
        sts_at(&endpoint, None).mint(&d, TTL).await.unwrap();
        assert!(!asked.lock().unwrap()[1].1.contains("RoleArn"));

        let (endpoint, _) = fake_sts("<ErrorResponse><Error><Code>AccessDenied</Code><Message>not authorized to assume</Message></Error></ErrorResponse>", 403).await;
        let e = sts_at(&endpoint, None).mint(&d, TTL).await.unwrap_err();
        assert_eq!(format!("{e:#}"), "STS AssumeRole answered HTTP 403: AccessDenied not authorized to assume");
    }

    #[test]
    fn the_session_policy_reaches_what_a_reader_needs() {
        let d: DriveId = CHECK_DRIVE.parse().unwrap();
        let p: serde_json::Value = serde_json::from_str(&session_policy("bkt", "pool/", &d)).unwrap();
        assert_eq!(p["Statement"][0]["Action"], serde_json::json!(["s3:GetObject"]));
        assert_eq!(
            p["Statement"][0]["Resource"],
            serde_json::json!(["arn:aws:s3:::bkt/pool/voidfs.json", "arn:aws:s3:::bkt/pool/shards/*", "arn:aws:s3:::bkt/pool/pages/*", format!("arn:aws:s3:::bkt/pool/drives/{d}/*")])
        );
        assert_eq!(p["Statement"][1]["Action"], serde_json::json!(["s3:ListBucket"]));
        assert_eq!(p["Statement"][1]["Resource"], serde_json::json!(["arn:aws:s3:::bkt"]));
        assert_eq!(p["Statement"][1]["Condition"]["StringLike"]["s3:prefix"], serde_json::json!([format!("pool/drives/{d}/*")]));
        let at_root: serde_json::Value = serde_json::from_str(&session_policy("bkt", "", &d)).unwrap();
        assert_eq!(at_root["Statement"][0]["Resource"][0], "arn:aws:s3:::bkt/voidfs.json");
    }

    #[test]
    fn an_assume_role_answer_is_parsed() {
        let ok = r#"<AssumeRoleResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/"><AssumeRoleResult><AssumedRoleUser><Arn>a</Arn></AssumedRoleUser>
            <Credentials><AccessKeyId>ASIAEXAMPLE</AccessKeyId><SecretAccessKey>secret</SecretAccessKey><SessionToken>token</SessionToken><Expiration>2026-10-04T20:30:16Z</Expiration></Credentials>
            </AssumeRoleResult></AssumeRoleResponse>"#;
        let m = parse_assume_role(ok).unwrap();
        assert_eq!((m.access_key_id.as_str(), m.secret_access_key.as_str(), m.session_token.as_deref()), ("ASIAEXAMPLE", "secret", Some("token")));
        assert_eq!(m.expires_at.to_rfc3339(), "2026-10-04T20:30:16+00:00");
        assert!(!format!("{m:?}").contains("secret") && !format!("{m:?}").contains("token"), "no secrets in Debug: {m:?}");
        assert!(parse_assume_role("<AssumeRoleResponse/>").is_err());
        let err = r#"<ErrorResponse xmlns="https://sts.amazonaws.com/doc/2011-06-15/"><Error><Type>Sender</Type><Code>AccessDenied</Code><Message>not authorized</Message></Error></ErrorResponse>"#;
        assert_eq!(sts_error(err).as_deref(), Some("AccessDenied not authorized"));
    }

    #[test]
    fn the_access_generation_follows_the_keys_that_reach_the_drive() {
        let key = |id: &str, scope, drives: Option<&[&str]>| KeyInfo { id: id.into(), secret: "s".into(), scope, drives: drives.map(|d| d.iter().map(|s| s.to_string()).collect()) };
        let mut keys = Keys::default();
        keys.insert(key("VFADMIN", Scope::Admin, None));
        keys.insert(key("VFREAD", Scope::Read, Some(&["footage"])));
        let g = access_generation(&keys, "footage", "d-1");
        assert!(g < 1 << 48);
        assert_eq!(access_generation(&keys, "footage", "d-1"), g, "the same rules, the same generation");
        assert_ne!(access_generation(&keys, "other", "d-2"), g, "the read key doesn't reach this drive");
        keys.insert(key("VFREAD", Scope::Write, Some(&["footage"])));
        assert_ne!(access_generation(&keys, "footage", "d-1"), g, "a scope changed");
        let mut other = Keys::default();
        other.insert(key("VFADMIN", Scope::Admin, None));
        keys.insert(key("VFREAD", Scope::Read, Some(&["elsewhere"])));
        assert_eq!(access_generation(&keys, "footage", "d-1"), access_generation(&other, "footage", "d-1"), "keys that don't reach the drive don't count");
    }
}
