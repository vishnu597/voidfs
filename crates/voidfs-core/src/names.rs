// SPDX-License-Identifier: Apache-2.0
//! Names, keys and their ordering (format §7.6, protocol §1).

/// Longest name segment, in bytes of UTF-8.
pub const MAX_NAME_BYTES: usize = 255;
/// Longest key, in bytes of UTF-8.
pub const MAX_KEY_BYTES: usize = 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NameError {
    #[error("name is empty")]
    Empty,
    #[error("name is longer than {MAX_NAME_BYTES} bytes")]
    TooLong,
    #[error("name contains '/' or NUL")]
    BadCharacter,
    #[error("name is '.' or '..'")]
    Dot,
    #[error("key is longer than {MAX_KEY_BYTES} bytes")]
    KeyTooLong,
    #[error("key starts with '/' or has an empty segment")]
    EmptySegment,
}

/// Checks one path segment.
pub fn validate_name(name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(NameError::TooLong);
    }
    if name.contains(['/', '\0']) {
        return Err(NameError::BadCharacter);
    }
    if name == "." || name == ".." {
        return Err(NameError::Dot);
    }
    Ok(())
}

/// The bytes a child is ordered by within its folder: the name, plus `/` for a folder.
/// A depth-first walk in this order yields keys in lexical order.
pub fn segment(name: &str, is_folder: bool) -> Vec<u8> {
    let mut s = Vec::with_capacity(name.len() + 1);
    s.extend_from_slice(name.as_bytes());
    if is_folder {
        s.push(b'/');
    }
    s
}

/// A parsed, validated key: the names from the root, and whether it names a folder.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Key {
    names: Vec<String>,
    folder: bool,
}

impl Key {
    /// Parses an S3 key. The empty key is the root folder.
    pub fn parse(key: &str) -> Result<Key, NameError> {
        if key.len() > MAX_KEY_BYTES {
            return Err(NameError::KeyTooLong);
        }
        if key.is_empty() {
            return Ok(Key { names: Vec::new(), folder: true });
        }
        let folder = key.ends_with('/');
        let body = if folder { &key[..key.len() - 1] } else { key };
        if body.is_empty() {
            return Err(NameError::EmptySegment);
        }
        let mut names = Vec::new();
        for part in body.split('/') {
            if part.is_empty() {
                return Err(NameError::EmptySegment);
            }
            validate_name(part)?;
            names.push(part.to_owned());
        }
        Ok(Key { names, folder })
    }

    pub fn is_root(&self) -> bool {
        self.names.is_empty()
    }

    pub fn is_folder(&self) -> bool {
        self.folder
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The last name, or `None` for the root.
    pub fn name(&self) -> Option<&str> {
        self.names.last().map(String::as_str)
    }

    /// The folders leading to this key, outermost first.
    pub fn parent_names(&self) -> &[String] {
        &self.names[..self.names.len().saturating_sub(1)]
    }

    /// Renders the key as S3 sees it.
    pub fn render(&self) -> String {
        let mut s = self.names.join("/");
        if self.folder && !s.is_empty() {
            s.push('/');
        }
        s
    }

    /// Whether `other` is this folder or inside it.
    pub fn contains(&self, other: &Key) -> bool {
        self.folder && other.names.len() >= self.names.len() && other.names[..self.names.len()] == self.names[..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_rules() {
        assert!(validate_name("été ✓.txt").is_ok());
        assert_eq!(validate_name(""), Err(NameError::Empty));
        assert_eq!(validate_name("a/b"), Err(NameError::BadCharacter));
        assert_eq!(validate_name("a\0"), Err(NameError::BadCharacter));
        assert_eq!(validate_name(".."), Err(NameError::Dot));
        assert_eq!(validate_name(&"x".repeat(256)), Err(NameError::TooLong));
        assert!(validate_name(&"x".repeat(255)).is_ok());
    }

    #[test]
    fn keys_parse_and_render() {
        let k = Key::parse("a/b/c.txt").unwrap();
        assert_eq!(k.names(), ["a", "b", "c.txt"]);
        assert!(!k.is_folder());
        assert_eq!(k.parent_names(), ["a", "b"]);
        assert_eq!(k.render(), "a/b/c.txt");
        let f = Key::parse("a/b/").unwrap();
        assert!(f.is_folder());
        assert_eq!(f.render(), "a/b/");
        assert!(f.contains(&k));
        assert!(!Key::parse("a/bc/").unwrap().contains(&k));
        assert!(Key::parse("").unwrap().is_root());
        for bad in ["/a", "a//b", "/", "a/./b", "a/../b"] {
            assert!(Key::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn segment_order_matches_key_order() {
        // "a.b/" sorts before "a/" as keys ('.' < '/'), so it must as segments too.
        let mut segs = vec![segment("a", true), segment("a.b", true), segment("a-c", false)];
        segs.sort();
        assert_eq!(segs, vec![b"a-c".to_vec(), b"a.b/".to_vec(), b"a/".to_vec()]);
        let mut keys = vec!["a/x", "a.b/x", "a-c"];
        keys.sort();
        assert_eq!(keys, vec!["a-c", "a.b/x", "a/x"]);
    }
}
