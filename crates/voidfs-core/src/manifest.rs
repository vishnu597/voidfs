// SPDX-License-Identifier: Apache-2.0
//! Manifest trees for content with more than 1,024 extents (format §5.1).
//!
//! Leaf boundaries are content-defined (a shard hash with 9 low zero bits, within 256–1,024
//! extents per leaf), so an edit that changes a few extents rewrites one leaf and the path to
//! the root, not the whole manifest.

use bytes::Bytes;

use crate::ids::ShardHash;
use crate::model::{ContentDescriptor, Extent, ManifestPage, PageRef};

/// Most extents or children in one page, and most extents inline in a descriptor.
pub const MAX_FANOUT: usize = 1024;
const MIN_FANOUT: usize = 256;

/// A page ready to store under `pages/`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Page {
    pub hash: ShardHash,
    pub bytes: Bytes,
}

impl Page {
    fn encode(page: &ManifestPage) -> Page {
        let bytes = Bytes::from(serde_json::to_vec(page).expect("pages always serialize"));
        Page { hash: ShardHash::of(&bytes), bytes }
    }
}

fn is_boundary(h: &ShardHash) -> bool {
    u16::from_be_bytes([h.0[30], h.0[31]]) & 0x01ff == 0
}

fn group<T>(items: Vec<T>, key: impl Fn(&T) -> Option<ShardHash>) -> Vec<Vec<T>> {
    let mut groups = Vec::new();
    let mut cur = Vec::new();
    for item in items {
        let cut = key(&item).is_some_and(|h| is_boundary(&h));
        cur.push(item);
        if cur.len() >= MAX_FANOUT || (cut && cur.len() >= MIN_FANOUT) {
            groups.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    groups
}

/// Describes content, building a tree when it has more than [`MAX_FANOUT`] extents.
/// Returns the descriptor and every page it needs (already-stored pages are harmless to
/// store again; they are content-addressed).
pub fn describe(extents: Vec<Extent>) -> (ContentDescriptor, Vec<Page>) {
    if extents.len() <= MAX_FANOUT {
        return (ContentDescriptor::Inline { extents }, Vec::new());
    }
    let size = extents.iter().map(Extent::len).sum();
    let mut pages = Vec::new();
    let mut level: Vec<PageRef> = group(extents, Extent::shard)
        .into_iter()
        .map(|leaf| {
            let size = leaf.iter().map(Extent::len).sum();
            let page = Page::encode(&ManifestPage::Leaf { extents: leaf });
            let r = PageRef { page: page.hash, size };
            pages.push(page);
            r
        })
        .collect();
    while level.len() > 1 {
        level = group(level, |r| Some(r.page))
            .into_iter()
            .map(|children| {
                let size = children.iter().map(|c| c.size).sum();
                let page = Page::encode(&ManifestPage::Node { children });
                let r = PageRef { page: page.hash, size };
                pages.push(page);
                r
            })
            .collect();
    }
    (ContentDescriptor::Tree { root: level[0].page, size }, pages)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest page {0} is missing")]
    MissingPage(ShardHash),
    #[error("manifest page {0} is corrupt: {1}")]
    Corrupt(ShardHash, String),
}

/// Expands a descriptor to its full extent list, fetching pages as needed.
pub fn flatten(
    desc: &ContentDescriptor,
    fetch: &mut impl FnMut(&ShardHash) -> Option<Bytes>,
) -> Result<Vec<Extent>, ManifestError> {
    match desc {
        ContentDescriptor::Inline { extents } => Ok(extents.clone()),
        ContentDescriptor::Tree { root, size } => {
            let mut out = Vec::new();
            let mut stack = vec![*root];
            while let Some(h) = stack.pop() {
                let bytes = fetch(&h).ok_or(ManifestError::MissingPage(h))?;
                if ShardHash::of(&bytes) != h {
                    return Err(ManifestError::Corrupt(h, "hash mismatch".into()));
                }
                match serde_json::from_slice(&bytes).map_err(|e| ManifestError::Corrupt(h, e.to_string()))? {
                    ManifestPage::Leaf { extents } => out.extend(extents),
                    ManifestPage::Node { children } => stack.extend(children.iter().rev().map(|c| c.page)),
                }
            }
            let total: u64 = out.iter().map(Extent::len).sum();
            if total != *size {
                return Err(ManifestError::Corrupt(*root, format!("tree holds {total} bytes, descriptor says {size}")));
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn extents(n: usize, salt: u8) -> Vec<Extent> {
        (0..n).map(|i| Extent::Shard { s: ShardHash::of(&[salt, (i >> 16) as u8, (i >> 8) as u8, i as u8]), n: 1000 + (i % 7) as u64 }).collect()
    }

    #[test]
    fn small_content_stays_inline() {
        let (d, pages) = describe(extents(MAX_FANOUT, 0));
        assert!(matches!(d, ContentDescriptor::Inline { .. }));
        assert!(pages.is_empty());
    }

    #[test]
    fn large_content_round_trips_through_a_tree() {
        let ext = extents(300_000, 1);
        let (d, pages) = describe(ext.clone());
        assert!(matches!(d, ContentDescriptor::Tree { .. }));
        assert_eq!(d.size(), ext.iter().map(Extent::len).sum::<u64>());
        let store: HashMap<_, _> = pages.iter().map(|p| (p.hash, p.bytes.clone())).collect();
        assert_eq!(flatten(&d, &mut |h| store.get(h).cloned()).unwrap(), ext);
        for p in &pages {
            match serde_json::from_slice::<ManifestPage>(&p.bytes).unwrap() {
                ManifestPage::Leaf { extents } => assert!(extents.len() <= MAX_FANOUT),
                ManifestPage::Node { children } => assert!(children.len() <= MAX_FANOUT),
            }
        }
    }

    #[test]
    fn an_edit_rewrites_few_pages() {
        let ext = extents(100_000, 2);
        let (_, before) = describe(ext.clone());
        let mut edited = ext.clone();
        edited[50_000] = Extent::Zero { z: 5 };
        let (_, after) = describe(edited);
        let old: std::collections::HashSet<_> = before.iter().map(|p| p.hash).collect();
        let fresh = after.iter().filter(|p| !old.contains(&p.hash)).count();
        assert!(fresh <= 4, "{fresh} pages changed of {}", after.len());
    }

    #[test]
    fn missing_or_corrupt_pages_are_errors() {
        let (d, pages) = describe(extents(5000, 3));
        assert!(matches!(flatten(&d, &mut |_| None), Err(ManifestError::MissingPage(_))));
        let mut store: HashMap<_, _> = pages.iter().map(|p| (p.hash, p.bytes.clone())).collect();
        let first = pages[0].hash;
        store.insert(first, Bytes::from_static(b"{}"));
        assert!(matches!(flatten(&d, &mut |h| store.get(h).cloned()), Err(ManifestError::Corrupt(..)) | Err(ManifestError::MissingPage(_))));
    }
}
