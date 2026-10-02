// SPDX-License-Identifier: Apache-2.0
//! `keys generate`: a key for `voidfs-server --key`, made here, since a self-hosted server has no
//! account to mint one (SpaceFS's `keys create` asks theirs).

use rand::RngExt;
use serde_json::json;

use crate::output::{Out, Result, stdout};

/// Key ids: `VF` and 18 of these (protocol §2).
const ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// Secrets: 40 of these, as `voidfs-server` makes its own.
const SECRET_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Get, head, list, and read history and the change feed
    Read,
    /// Everything in read, plus every object mutation
    Write,
    /// Everything in write, plus creating, forking and deleting drives
    Admin,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Admin => "admin",
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The id and the secret, labelled, and the server's flag
    Text,
    /// `export` lines for a shell: `eval "$(void keys generate --format env)"`
    Env,
    /// The key as JSON, which --key-file reads
    Json,
}

fn random(alphabet: &[u8], n: usize) -> String {
    let mut rng = rand::rng();
    (0..n).map(|_| alphabet[rng.random_range(0..alphabet.len())] as char).collect()
}

/// A new key's id and secret.
pub fn new_key() -> (String, String) {
    (format!("VF{}", random(ID_ALPHABET, 18)), random(SECRET_ALPHABET, 40))
}

pub fn generate(out: Out, scope: Scope, format: Format) -> Result<()> {
    let (id, secret) = new_key();
    let scope = scope.as_str();
    let server_key = format!("{id}:{secret}:{scope}");
    let format = if out.json { Format::Json } else { format };
    match format {
        Format::Json => Out { json: true }.emit(&json!({ "accessKeyId": id, "secretAccessKey": secret, "scope": scope, "serverKey": server_key }), String::new),
        Format::Env => stdout(&format!("export VOIDFS_ACCESS_KEY_ID='{id}'\nexport VOIDFS_SECRET_ACCESS_KEY='{secret}'")),
        Format::Text => stdout(&format!(
            "access key id:     {id}\nsecret access key: {secret}\nscope:             {scope}\n\nGive it to the server, which keeps no keys of its own:\n  voidfs-server --key {server_key} …\n(or VOIDFS_KEYS={server_key}; several keys are separated by commas)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_have_the_protocols_form() {
        let (id, secret) = new_key();
        assert_eq!(id.len(), 20);
        assert!(id.starts_with("VF") && id[2..].bytes().all(|b| ID_ALPHABET.contains(&b)), "{id}");
        assert_eq!(secret.len(), 40);
        assert!(secret.bytes().all(|b| SECRET_ALPHABET.contains(&b)));
        assert_ne!(new_key(), new_key());
    }
}
