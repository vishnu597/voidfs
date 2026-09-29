// SPDX-License-Identifier: Apache-2.0
//! Virtual-host addressing (protocol §2): `<drive>.<domain>/<key>` beside `/<drive>/<key>`.

use http::{HeaderMap, Uri};

/// The deployment's domains. A request to `<drive>.<domain>` names that drive; a request to a
/// domain itself, or to any other host, is path-style.
#[derive(Clone, Debug, Default)]
pub struct Domains(Vec<String>);

impl Domains {
    pub fn new(domains: impl IntoIterator<Item = String>) -> Domains {
        let mut d: Vec<String> = domains.into_iter().collect();
        // Longest first, so that with `example.com` and `s3.example.com`, `a.s3.example.com` is
        // drive `a`, and `s3.example.com` itself is path-style.
        d.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        d.dedup();
        Domains(d)
    }

    /// The drive a request's host names, or `None` for path-style addressing.
    pub fn drive(&self, uri: &Uri, headers: &HeaderMap) -> Option<String> {
        if self.0.is_empty() {
            return None;
        }
        // HTTP/2 carries the host in `:authority` rather than a `Host` header.
        let host = match headers.get(http::header::HOST) {
            Some(h) => h.to_str().ok()?,
            None => uri.authority()?.as_str(),
        };
        self.drive_of(host)
    }

    fn drive_of(&self, host: &str) -> Option<String> {
        if host.starts_with('[') {
            return None;
        }
        let host = host.split_once(':').map_or(host, |(h, _)| h);
        let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
        for d in &self.0 {
            if host == *d {
                return None;
            }
            if let Some(drive) = host.strip_suffix(d.as_str()).and_then(|p| p.strip_suffix('.'))
                && !drive.is_empty() {
                    return Some(drive.to_owned());
                }
        }
        None
    }
}

/// `--virtual-host-domain`: a DNS name, compared without case or a trailing dot.
pub fn parse_domain(s: &str) -> Result<String, String> {
    let d = s.trim().strip_suffix('.').unwrap_or(s.trim()).to_ascii_lowercase();
    let valid = !d.is_empty()
        && d.split('.').all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    if valid { Ok(d) } else { Err(format!("{s:?} is not a domain name such as voidfs.example.com (no scheme, port or *.)")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domains(d: &[&str]) -> Domains {
        Domains::new(d.iter().map(|s| parse_domain(s).unwrap()))
    }

    fn drive(d: &Domains, host: &str) -> Option<String> {
        let mut h = HeaderMap::new();
        h.insert(http::header::HOST, host.parse().unwrap());
        d.drive(&Uri::from_static("/k"), &h)
    }

    #[test]
    fn hosts_name_drives() {
        let d = domains(&["voidfs.example.com"]);
        assert_eq!(drive(&d, "footage.voidfs.example.com").as_deref(), Some("footage"));
        assert_eq!(drive(&d, "footage.voidfs.example.com:9000").as_deref(), Some("footage"));
        assert_eq!(drive(&d, "Footage.VoidFS.Example.COM").as_deref(), Some("footage"));
        assert_eq!(drive(&d, "footage.voidfs.example.com.").as_deref(), Some("footage"));
        assert_eq!(drive(&d, "footage.voidfs.example.com.:443").as_deref(), Some("footage"));
        assert_eq!(drive(&d, "a.b.voidfs.example.com").as_deref(), Some("a.b"), "drive names may contain dots");
        let id = "d-0192f8a4-3b6c-7d2e-9f10-1a2b3c4d5e6f";
        assert_eq!(drive(&d, &format!("{id}.voidfs.example.com")).as_deref(), Some(id));
    }

    #[test]
    fn other_hosts_are_path_style() {
        let d = domains(&["voidfs.example.com"]);
        assert_eq!(drive(&d, "voidfs.example.com"), None);
        assert_eq!(drive(&d, "voidfs.example.com:9000"), None);
        assert_eq!(drive(&d, "VOIDFS.example.com."), None);
        assert_eq!(drive(&d, ".voidfs.example.com"), None);
        assert_eq!(drive(&d, "xvoidfs.example.com"), None);
        assert_eq!(drive(&d, "example.com"), None);
        assert_eq!(drive(&d, "127.0.0.1:9000"), None);
        assert_eq!(drive(&d, "[::1]:9000"), None);
        assert_eq!(drive(&d, "localhost"), None);
        assert_eq!(drive(&Domains::default(), "footage.voidfs.example.com"), None, "no domains: always path-style");
    }

    #[test]
    fn the_longest_domain_wins() {
        let d = domains(&["example.com", "s3.example.com"]);
        assert_eq!(drive(&d, "a.s3.example.com").as_deref(), Some("a"));
        assert_eq!(drive(&d, "s3.example.com"), None);
        assert_eq!(drive(&d, "a.example.com").as_deref(), Some("a"));
        assert_eq!(drive(&d, "example.com"), None);
    }

    #[test]
    fn http2_authority_names_the_drive() {
        let d = domains(&["localhost"]);
        let uri: Uri = "http://footage.localhost:9000/k".parse().unwrap();
        assert_eq!(d.drive(&uri, &HeaderMap::new()).as_deref(), Some("footage"));
        let mut h = HeaderMap::new();
        h.insert(http::header::HOST, "other.localhost".parse().unwrap());
        assert_eq!(d.drive(&uri, &h).as_deref(), Some("other"), "the Host header wins, as in signing");
    }

    #[test]
    fn domains_parse() {
        assert_eq!(parse_domain("VoidFS.Example.com.").unwrap(), "voidfs.example.com");
        assert_eq!(parse_domain("localhost").unwrap(), "localhost");
        for bad in ["", ".", "*.example.com", "example.com:9000", "https://example.com", "a..b", "exa mple.com"] {
            assert!(parse_domain(bad).is_err(), "{bad:?}");
        }
    }
}
