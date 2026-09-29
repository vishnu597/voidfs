// SPDX-License-Identifier: Apache-2.0
//! Executes cases: builds and signs each request, sends it, and checks the response.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings, UriPathNormalizationMode, sign};
use aws_sigv4::sign::v4;
use base64::Engine;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use rand::RngExt;
use serde_json::Value;

use crate::cases::{Body, BodyExpect, Case, Step, seeded_bytes};
use crate::check::{self, Subject, Vars, subst};

/// RFC 3986 unreserved characters stay; everything else is encoded (as SigV4 requires).
const QUERY: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

#[derive(Clone, Debug)]
pub struct Key {
    pub id: String,
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail { step: usize, message: String },
    Skip(String),
}

#[derive(Debug, Clone)]
pub struct CaseResult {
    pub id: String,
    pub outcome: Outcome,
    pub elapsed: Duration,
}

pub struct Runner {
    client: reqwest::Client,
    endpoint: String,
    authority: String,
    keys: HashMap<String, Key>,
    /// Print every request and response while running.
    pub verbose: bool,
    /// Send `/<drive>/<key>` as `/<key>` to host `<drive>.<domain>` (virtual-host addressing),
    /// still connecting to the endpoint. `/` goes to `<domain>` itself.
    pub virtual_host: Option<String>,
}

struct Response {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Runner {
    /// `keys` must contain `admin`; other names satisfy `key:<name>` requirements.
    pub fn new(endpoint: &str, keys: HashMap<String, Key>) -> anyhow::Result<Runner> {
        anyhow::ensure!(keys.contains_key("admin"), "an admin key is required");
        let endpoint = endpoint.trim_end_matches('/').to_owned();
        let url: http::Uri = endpoint.parse()?;
        let authority = url.authority().ok_or_else(|| anyhow::anyhow!("endpoint has no host"))?.to_string();
        let client = reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?;
        Ok(Runner { client, endpoint, authority, keys, verbose: false, virtual_host: None })
    }

    pub async fn run_case(&self, case: &Case) -> CaseResult {
        let started = Instant::now();
        for r in &case.requires {
            let have = r.strip_prefix("key:").is_some_and(|k| self.keys.contains_key(k));
            if !have {
                return CaseResult { id: case.id.clone(), outcome: Outcome::Skip(format!("needs {r}")), elapsed: started.elapsed() };
            }
        }
        let mut vars: Vars = HashMap::new();
        for v in ["drive", "drive2", "drive3"] {
            vars.insert(v.into(), fresh_drive_name());
        }
        let mut outcome = Outcome::Pass;
        for (i, step) in case.steps.iter().enumerate() {
            if let Err(message) = self.step(step, &mut vars).await {
                outcome = Outcome::Fail { step: i, message };
                break;
            }
        }
        self.cleanup(&vars).await;
        CaseResult { id: case.id.clone(), outcome, elapsed: started.elapsed() }
    }

    async fn step(&self, step: &Step, vars: &mut Vars) -> Result<(), String> {
        let key = self.keys.get(&step.key).ok_or_else(|| format!("no key named {}", step.key))?;
        let req = &step.request;
        let path = subst(&req.path, vars)?;
        let mut query = Vec::new();
        for (name, raw) in &req.query {
            let k = utf8_percent_encode(name, QUERY).to_string();
            query.push(if raw.is_empty() { k } else { format!("{k}={}", utf8_percent_encode(&subst(raw, vars)?, QUERY)) });
        }
        let mut headers: Vec<(String, String)> = Vec::new();
        for (k, v) in &req.headers {
            headers.push((k.to_ascii_lowercase(), subst(v, vars)?));
        }
        let body = match &req.body {
            None => Vec::new(),
            Some(Body::Text(t)) => subst(t, vars)?.into_bytes(),
            Some(Body::Base64(b)) => base64::engine::general_purpose::STANDARD.decode(b).map_err(|e| e.to_string())?,
            Some(Body::Bytes { seed, size }) => seeded_bytes(*seed, *size),
            Some(Body::Patch(edits)) => {
                let edits: Vec<_> = edits.iter().map(|(o, t)| voidfs_core::patch::Edit { offset: *o, data: t.as_bytes() }).collect();
                voidfs_core::patch::encode(&edits)
            }
            Some(Body::Json(v)) => {
                if !headers.iter().any(|(k, _)| k == "content-type") {
                    headers.push(("content-type".into(), "application/json".into()));
                }
                serde_json::to_vec(v).unwrap()
            }
        };
        let unsigned: Vec<String> = req.unsigned_headers.iter().map(|h| h.to_ascii_lowercase()).collect();
        let resp = self.send(&req.method, &path, &query.join("&"), headers, &unsigned, body, key).await?;
        if self.verbose {
            eprintln!("    {} {}{} -> {} {}", req.method, path, if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) }, resp.status, String::from_utf8_lossy(&resp.body).chars().take(300).collect::<String>());
        }
        self.expect(step, &resp, vars)
    }

    #[allow(clippy::too_many_arguments)]
    async fn send(
        &self,
        method: &str,
        path: &str,
        query: &str,
        mut headers: Vec<(String, String)>,
        unsigned: &[String],
        body: Vec<u8>,
        key: &Key,
    ) -> Result<Response, String> {
        let (host, path) = self.address(path);
        let uri = if query.is_empty() { format!("{}{}", self.endpoint, path) } else { format!("{}{}?{}", self.endpoint, path, query) };
        headers.push(("host".into(), host));
        let (signed, extra): (Vec<_>, Vec<_>) = headers.into_iter().partition(|(k, _)| !unsigned.contains(k));

        let identity = Credentials::new(&key.id, &key.secret, None, None, "voidfs-conformance").into();
        let mut settings = SigningSettings::default();
        settings.percent_encoding_mode = PercentEncodingMode::Single;
        settings.uri_path_normalization_mode = UriPathNormalizationMode::Disabled;
        settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        let params = v4::SigningParams::builder()
            .identity(&identity)
            .region("us-east-1")
            .name("s3")
            .time(SystemTime::now())
            .settings(settings)
            .build()
            .map_err(|e| e.to_string())?
            .into();
        let signable = SignableRequest::new(method, &uri, signed.iter().map(|(k, v)| (k.as_str(), v.as_str())), SignableBody::Bytes(&body))
            .map_err(|e| format!("cannot sign: {e}"))?;
        let (instructions, _) = sign(signable, &params).map_err(|e| format!("cannot sign: {e}"))?.into_parts();

        let mut builder = http::Request::builder().method(method).uri(&uri);
        for (k, v) in signed.iter().chain(extra.iter()) {
            builder = builder.header(k, v);
        }
        let mut request = builder.body(body).map_err(|e| e.to_string())?;
        instructions.apply_to_request_http1x(&mut request);
        let request = reqwest::Request::try_from(request).map_err(|e| e.to_string())?;
        let resp = self.client.execute(request).await.map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_ascii_lowercase(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect();
        let body = resp.bytes().await.map_err(|e| format!("reading body: {e}"))?.to_vec();
        Ok(Response { status, headers, body })
    }

    /// The host and path a path-style request is sent with.
    fn address(&self, path: &str) -> (String, String) {
        let Some(domain) = &self.virtual_host else { return (self.authority.clone(), path.to_owned()) };
        let port = self.authority.rsplit_once(':').filter(|(_, p)| p.bytes().all(|b| b.is_ascii_digit())).map(|(_, p)| format!(":{p}")).unwrap_or_default();
        let rest = path.strip_prefix('/').unwrap_or(path);
        if rest.is_empty() {
            return (format!("{domain}{port}"), "/".into());
        }
        let (drive, key) = rest.split_once('/').unwrap_or((rest, ""));
        (format!("{drive}.{domain}{port}"), format!("/{key}"))
    }

    fn expect(&self, step: &Step, resp: &Response, vars: &mut Vars) -> Result<(), String> {
        let exp = &step.expect;
        let brief = || {
            let text = String::from_utf8_lossy(&resp.body);
            let t: String = text.chars().take(200).collect();
            if t.is_empty() { String::new() } else { format!(": {t}") }
        };
        if !exp.status.accepts(resp.status) {
            return Err(format!("status {} but expected {}{}", resp.status, exp.status, brief()));
        }
        for (name, m) in &exp.headers {
            let v = resp.headers.get(&name.to_ascii_lowercase()).map(|s| Value::String(s.clone()));
            check::check(m, Subject::Value(v), vars).map_err(|e| format!("header {name} {e}"))?;
        }
        if let Some(b) = &exp.body {
            check_body(b, &resp.body, vars)?;
        }
        Ok(())
    }

    async fn cleanup(&self, vars: &Vars) {
        let admin = &self.keys["admin"];
        for v in ["drive", "drive2", "drive3"] {
            let path = format!("/{}", vars[v]);
            let headers = vec![("x-voidfs-hard-delete".to_string(), "true".to_string())];
            let _ = self.send("DELETE", &path, "", headers, &[], Vec::new(), admin).await;
        }
    }
}

fn check_body(b: &BodyExpect, body: &[u8], vars: &mut Vars) -> Result<(), String> {
    if let Some(t) = &b.text {
        let want = subst(t, vars)?;
        if body != want.as_bytes() {
            return Err(format!("body is {:?} but expected {:?}", String::from_utf8_lossy(body).chars().take(200).collect::<String>(), want));
        }
    }
    if let Some(b64) = &b.base64 {
        let want = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| e.to_string())?;
        if body != want.as_slice() {
            return Err(format!("body is {} but expected {b64}", base64::engine::general_purpose::STANDARD.encode(body)));
        }
    }
    if let Some(n) = b.size
        && body.len() as u64 != n {
            return Err(format!("body is {} bytes, expected {n}", body.len()));
        }
    if b.s3_error.is_some() || !b.xml.is_empty() {
        let text = std::str::from_utf8(body).map_err(|_| "body is not UTF-8 XML".to_string())?;
        let doc = roxmltree::Document::parse(text).map_err(|e| format!("body is not XML ({e}): {}", text.chars().take(200).collect::<String>()))?;
        if let Some(code) = &b.s3_error {
            let got = check::xml_path(&doc, "Error/Code");
            if got.first() != Some(code) {
                return Err(format!("S3 error code is {got:?}, expected {code}"));
            }
        }
        for m in &b.xml {
            let path = m.path.as_deref().unwrap_or_default();
            let found = check::xml_path(&doc, path);
            check::check(m, Subject::Xml(&found), vars).map_err(|e| format!("XML {path} {e}"))?;
        }
    }
    if !b.json.is_empty() {
        let v: Value = serde_json::from_slice(body).map_err(|e| format!("body is not JSON ({e})"))?;
        for m in &b.json {
            let path = m.path.as_deref().unwrap_or("$");
            let found = check::json_path(&v, path)?;
            check::check(m, Subject::Value(found), vars).map_err(|e| format!("JSON {path} {e}"))?;
        }
    }
    Ok(())
}

fn fresh_drive_name() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    let suffix: String = (0..10).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect();
    format!("vfc-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_host_addresses() {
        let keys = HashMap::from([("admin".to_string(), Key { id: "a".into(), secret: "s".into() })]);
        let mut r = Runner::new("http://127.0.0.1:9100", keys).unwrap();
        assert_eq!(r.address("/d/k%20x"), ("127.0.0.1:9100".into(), "/d/k%20x".into()));
        r.virtual_host = Some("s3.localhost".into());
        assert_eq!(r.address("/"), ("s3.localhost:9100".into(), "/".into()));
        assert_eq!(r.address("/d"), ("d.s3.localhost:9100".into(), "/".into()));
        assert_eq!(r.address("/d/"), ("d.s3.localhost:9100".into(), "/".into()));
        assert_eq!(r.address("/d/a/b%20c"), ("d.s3.localhost:9100".into(), "/a/b%20c".into()));
    }
}
