// SPDX-License-Identifier: Apache-2.0
//! Executes cases: builds and signs each request, sends it, and checks the response.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine;
use percent_encoding::utf8_percent_encode;
use rand::RngExt;
use serde_json::Value;
use voidfs_sdk::sign::{PATH, QUERY, Signer};

use crate::cases::{Body, BodyExpect, BucketRequest, Case, Step, Upload};
use crate::check::{self, Subject, Vars, subst};

/// Where the runner keeps the case's last storage credentials (protocol §5.5): not a name a case
/// can use as a variable.
const CREDENTIALS: &str = "storage:credentials";

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
    keys: HashMap<String, Key>,
    /// Print every request and response while running.
    pub verbose: bool,
    /// Send `/<drive>/<key>` as `/<key>` to host `<drive>.<domain>` (virtual-host addressing),
    /// still connecting to the endpoint. `/` goes to `<domain>` itself.
    pub virtual_host: Option<String>,
    /// The pool features the server's drives have, for cases that require `feature:<name>`.
    pub features: Vec<String>,
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
        Signer::new(&endpoint, "", "")?; // checks the endpoint
        let client = reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?;
        Ok(Runner { client, endpoint, keys, verbose: false, virtual_host: None, features: Vec::new() })
    }

    pub async fn run_case(&self, case: &Case) -> CaseResult {
        let started = Instant::now();
        for r in &case.requires {
            let have = r.strip_prefix("key:").is_some_and(|k| self.keys.contains_key(k)) || r.strip_prefix("feature:").is_some_and(|f| self.features.iter().any(|g| g == f));
            if !have {
                return CaseResult { id: case.id.clone(), outcome: Outcome::Skip(format!("needs {r}")), elapsed: started.elapsed() };
            }
        }
        let mut vars: Vars = HashMap::new();
        for v in ["drive", "drive2", "drive3"] {
            vars.insert(v.into(), fresh_drive_name());
        }
        let nonce: u64 = rand::rng().random();
        let mut outcome = Outcome::Pass;
        for (i, step) in case.steps.iter().enumerate() {
            match self.step(step, &mut vars, nonce).await {
                Ok(Some(skip)) => {
                    outcome = Outcome::Skip(skip);
                    break;
                }
                Ok(None) => {}
                Err(message) => {
                    outcome = Outcome::Fail { step: i, message };
                    break;
                }
            }
        }
        self.cleanup(&vars).await;
        CaseResult { id: case.id.clone(), outcome, elapsed: started.elapsed() }
    }

    /// Runs a step; `Some` with why when it ends the case as skipped.
    async fn step(&self, step: &Step, vars: &mut Vars, nonce: u64) -> Result<Option<String>, String> {
        if let Some(u) = &step.upload {
            return self.upload(u, step, vars, nonce).await.map(|()| None);
        }
        if let Some(b) = &step.bucket {
            return self.bucket(b, step, vars, nonce).await.map(|()| None);
        }
        let key = self.keys.get(&step.key).ok_or_else(|| format!("no key named {}", step.key))?;
        let req = step.request.as_ref().ok_or("a step needs a request or an upload")?;
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
            Some(Body::Bytes(c)) => c.bytes(nonce),
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
            Some(Body::Plan(c)) => {
                headers.push(("content-type".into(), "application/json".into()));
                serde_json::to_vec(&serde_json::json!({ "shards": listed(&c.shards(nonce)) })).unwrap()
            }
            Some(Body::Commit(f)) => {
                headers.push(("content-type".into(), "application/json".into()));
                let c = f.content();
                let digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(c.bytes(nonce)));
                serde_json::to_vec(&serde_json::json!({ "token": subst(&f.token, vars)?, "size": c.size, "contentSha256": digest, "shards": listed(&c.shards(nonce)) })).unwrap()
            }
        };
        let unsigned: Vec<String> = req.unsigned_headers.iter().map(|h| h.to_ascii_lowercase()).collect();
        let resp = self.send(&req.method, &path, &query.join("&"), headers, &unsigned, body, key).await?;
        if self.verbose {
            eprintln!("    {} {}{} -> {} {}", req.method, path, if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) }, resp.status, String::from_utf8_lossy(&resp.body).chars().take(300).collect::<String>());
        }
        if let Some(s) = step.skip_if.as_ref().filter(|s| s.status == resp.status) {
            return Ok(Some(s.reason.clone()));
        }
        if resp.status == 200 && req.query.contains_key("x-voidfs-credentials") {
            vars.insert(CREDENTIALS.into(), String::from_utf8_lossy(&resp.body).into_owned());
        }
        self.expect(step, &resp, vars, nonce)?;
        Ok(None)
    }

    /// Sends a request to the storage the case's last storage credentials describe (protocol
    /// §5.5): path-style at their endpoint, under their root, signed with them.
    async fn bucket(&self, b: &BucketRequest, step: &Step, vars: &mut Vars, nonce: u64) -> Result<(), String> {
        let c: Value = serde_json::from_str(vars.get(CREDENTIALS).ok_or("no storage credentials: a bucket step needs an earlier 200 answer to ?x-voidfs-credentials")?)
            .map_err(|e| format!("the storage credentials are not JSON: {e}"))?;
        let st = &c["storage"];
        let field = |v: &Value, name: &str| v[name].as_str().map(str::to_owned).ok_or_else(|| format!("the storage credentials have no {name}"));
        let (endpoint, bucket, root) = (field(st, "endpoint")?, field(st, "bucket")?, st["root"].as_str().unwrap_or_default().to_owned());
        let cr = &st["credentials"];
        let region = st["region"].as_str().filter(|r| !r.is_empty()).unwrap_or("us-east-1");
        let signer = Signer::new(&endpoint, &field(cr, "accessKeyId")?, &field(cr, "secretAccessKey")?)
            .map_err(|e| e.to_string())?
            .with_region(region)
            .with_session_token(cr["sessionToken"].as_str());
        let bucket_path = format!("/{}", utf8_percent_encode(&bucket, QUERY));
        let (path, query) = match (&b.path, &b.shard, &b.list) {
            (Some(p), None, None) => (format!("{bucket_path}/{}", utf8_percent_encode(&format!("{root}{}", subst(p, vars)?), PATH)), String::new()),
            (None, Some(content), None) => {
                let shards = content.shards(nonce);
                let [shard] = shards.as_slice() else { return Err(format!("the content is {} shards, not one", shards.len())) };
                (format!("{bucket_path}/{}", utf8_percent_encode(&format!("{root}shards/{}", shard.hash.object_path()), PATH)), String::new())
            }
            (None, None, Some(prefix)) => (bucket_path, format!("delimiter=%2F&list-type=2&prefix={}", utf8_percent_encode(&format!("{root}{}", subst(prefix, vars)?), QUERY))),
            _ => return Err("a bucket step names one of path, shard and list".into()),
        };
        let body = match &b.body {
            None => Vec::new(),
            Some(Body::Text(t)) => subst(t, vars)?.into_bytes(),
            Some(Body::Bytes(c)) => c.bytes(nonce),
            Some(_) => return Err("a bucket step's body is text or bytes".into()),
        };
        let request = signer.sign(&b.method, &path, &query, Vec::new(), &[], body.into()).map_err(|e| e.to_string())?;
        let resp = self.execute(reqwest::Request::try_from(request).map_err(|e| e.to_string())?).await?;
        if self.verbose {
            eprintln!("    {} <bucket>{path}{} -> {} {}", b.method, if query.is_empty() { String::new() } else { format!("?{query}") }, resp.status, String::from_utf8_lossy(&resp.body).chars().take(300).collect::<String>());
        }
        self.expect(step, &resp, vars, nonce)
    }

    /// PUTs each shard a plan lists to its URL, with exactly its headers and no signature
    /// (protocol §4.11), and checks each answer's status.
    async fn upload(&self, u: &Upload, step: &Step, vars: &Vars, nonce: u64) -> Result<(), String> {
        let list: Value = serde_json::from_str(&subst(&u.list, vars)?).map_err(|e| format!("the upload list is not JSON: {e}"))?;
        let list = list.as_array().ok_or("the upload list is not a JSON array")?;
        let shards = u.content.shards(nonce);
        for (i, entry) in list.iter().enumerate() {
            if u.skip.contains(&i) {
                continue;
            }
            let hash = entry["hash"].as_str().ok_or_else(|| format!("upload[{i}] has no hash"))?;
            let url = entry["url"].as_str().ok_or_else(|| format!("upload[{i}] has no url"))?;
            let shard = shards.iter().find(|s| s.hash.to_hex() == hash).ok_or_else(|| format!("upload[{i}] names shard {hash}, which the content does not have"))?;
            let mut body = shard.bytes.to_vec();
            if u.corrupt {
                *body.last_mut().expect("a shard has bytes") ^= 0xff;
            }
            let mut req = self.client.put(url).body(body);
            for (k, v) in entry["headers"].as_object().into_iter().flatten() {
                req = req.header(k, v.as_str().ok_or_else(|| format!("upload[{i}] header {k} is not a string"))?);
            }
            let resp = req.send().await.map_err(|e| format!("upload[{i}] failed: {e}"))?;
            let status = resp.status().as_u16();
            if self.verbose {
                eprintln!("    PUT <upload[{i}]> -> {status}");
            }
            if !step.expect.status.accepts(status) {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!("upload[{i}]: status {status} but expected {}: {}", step.expect.status, text.chars().take(200).collect::<String>()));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn send(
        &self,
        method: &str,
        path: &str,
        query: &str,
        headers: Vec<(String, String)>,
        unsigned: &[String],
        body: Vec<u8>,
        key: &Key,
    ) -> Result<Response, String> {
        let mut signer = Signer::new(&self.endpoint, &key.id, &key.secret).map_err(|e| e.to_string())?;
        signer.virtual_host = self.virtual_host.clone();
        let request = signer.sign(method, path, query, headers, unsigned, body.into()).map_err(|e| e.to_string())?;
        self.execute(reqwest::Request::try_from(request).map_err(|e| e.to_string())?).await
    }

    async fn execute(&self, request: reqwest::Request) -> Result<Response, String> {
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

    fn expect(&self, step: &Step, resp: &Response, vars: &mut Vars, nonce: u64) -> Result<(), String> {
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
            check_body(b, &resp.body, vars, nonce)?;
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

fn check_body(b: &BodyExpect, body: &[u8], vars: &mut Vars, nonce: u64) -> Result<(), String> {
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
    if let Some(c) = &b.bytes
        && body != c.bytes(nonce).as_slice() {
            return Err(format!("body is {} bytes that are not the content {c:?}", body.len()));
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

/// A plan's or commit's shard list (protocol §4.11).
fn listed(shards: &[voidfs_core::chunk::Shard]) -> Vec<Value> {
    shards.iter().map(|s| serde_json::json!({ "hash": s.hash.to_hex(), "length": s.bytes.len() })).collect()
}

fn fresh_drive_name() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    let suffix: String = (0..10).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect();
    format!("vfc-{suffix}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_token_cases_other_shard_list_differs_whatever_the_nonce() {
        let suite = crate::cases::load(crate::CASES_JSON).unwrap();
        let case = suite.cases.iter().find(|c| c.id == "direct-upload-token").unwrap();
        let step = case.steps.iter().find(|s| s.name.as_deref() == Some("another shard list")).unwrap();
        let Some(Body::Commit(form)) = &step.request.as_ref().unwrap().body else { panic!("a commit") };
        let (offset, text) = form.edit.clone().unwrap();
        let end = offset as usize + text.len();
        // The seeded bytes are the same prefix whatever the size, so the edited range is enough.
        for nonce in 0..4096u64 {
            let before = crate::cases::seeded_bytes(form.seed ^ nonce, end as u64);
            assert_ne!(&before[offset as usize..end], text.as_bytes(), "with nonce {nonce} the edit changes nothing, so the commit would be accepted");
        }
    }

    #[test]
    fn held_count_case_accepts_an_edit_of_a_single_shard_but_still_checks_dedup() {
        let suite = crate::cases::load(crate::CASES_JSON).unwrap();
        let case = suite.cases.iter().find(|c| c.id == "direct-upload-held-counts").unwrap();
        let Some(Body::Bytes(original)) = &case.steps[1].request.as_ref().unwrap().body else { panic!("original content") };
        let Some(Body::Plan(edited)) = &case.steps[3].request.as_ref().unwrap().body else { panic!("edited content") };
        // This legitimate per-case nonce makes the 6 MiB input one shard. An edit must change
        // its only hash, so a correct plan cannot report any unchanged shard as held.
        let nonce = 77;
        let original = original.shards(nonce);
        let edited = edited.shards(nonce);
        assert_eq!(original.len(), 1);
        assert_eq!(edited.len(), 1);
        assert_ne!(original[0].hash, edited[0].hash);
        let held = edited.iter().filter(|s| original.iter().any(|o| o.hash == s.hash)).count();
        assert_eq!(held, 0);

        let unchanged_expect = case.steps[2].expect.body.as_ref().unwrap();
        let edited_expect = case.steps[3].expect.body.as_ref().unwrap();
        let check = |expect, response| check_body(expect, &serde_json::to_vec(&response).unwrap(), &mut Vars::new(), nonce);
        assert!(check(unchanged_expect, json!({ "held": original.len(), "upload": [] })).is_ok());
        assert!(check(unchanged_expect, json!({ "held": 0, "upload": [] })).is_err(), "the unchanged plan must still hold its existing shard");
        assert!(check(edited_expect, json!({ "held": held, "upload": listed(&edited), "token": "fixture" })).is_ok(), "a correct single-shard edit plan must satisfy the real case matchers");
        assert!(check(edited_expect, json!({ "held": held, "upload": [], "token": "fixture" })).is_err(), "the edited plan must still require an upload");
    }
}
