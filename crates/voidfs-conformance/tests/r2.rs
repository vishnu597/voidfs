// SPDX-License-Identifier: Apache-2.0
//! Storage-credentials conformance through the R2 API mint, with its requested scope enforced.

use std::collections::HashMap;

use voidfs_conformance::runner::{Key, Outcome, Runner};
use voidfs_conformance::{CASES_JSON, cases};
use voidfs_server::sigv4::{KeyInfo, Scope};
use voidfs_server::test_server::{ADMIN_KEY_ID, ADMIN_SECRET, Rules, TestServer};

#[tokio::test(flavor = "multi_thread")]
async fn r2_passes_the_storage_credentials_conformance_cases() {
    let read = KeyInfo { id: "VFR2READKEY".into(), secret: "fake-read-secret".into(), scope: Scope::Read, drives: None };
    let keys = HashMap::from([
        ("admin".into(), Key { id: ADMIN_KEY_ID.into(), secret: ADMIN_SECRET.into() }),
        ("read".into(), Key { id: read.id.clone(), secret: read.secret.clone() }),
    ]);
    let server = TestServer::with_r2_storage_credentials(vec![read], Rules::default()).await.unwrap();
    let runner = Runner::new(&server.endpoint, keys).unwrap();
    let suite = cases::load(CASES_JSON).unwrap();
    let selected: Vec<_> = suite.cases.iter().filter(|c| c.id.starts_with("storage-credentials-")).collect();
    assert_eq!(selected.len(), 5);
    let mut passed = 0;
    for case in selected {
        let result = runner.run_case(case).await;
        if case.id == "storage-credentials-not-offered" {
            assert!(matches!(result.outcome, Outcome::Skip(_)), "{}: {:?}", result.id, result.outcome);
        } else {
            assert_eq!(result.outcome, Outcome::Pass, "{}", result.id);
            passed += 1;
        }
    }
    assert_eq!(passed, 4);
    let usage = server.bucket.as_ref().unwrap().credential_use();
    assert!(usage.minted >= 5 && usage.reads > 0 && usage.lists > 0 && usage.refused >= 4, "{usage:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn r2_with_unrestricted_listings_does_not_offer_credentials() {
    let keys = HashMap::from([("admin".into(), Key { id: ADMIN_KEY_ID.into(), secret: ADMIN_SECRET.into() })]);
    let server = TestServer::with_r2_storage_credentials(Vec::new(), Rules { list_scope: false, ..Rules::default() }).await.unwrap();
    let runner = Runner::new(&server.endpoint, keys).unwrap();
    let suite = cases::load(CASES_JSON).unwrap();
    let case = suite.cases.iter().find(|c| c.id == "storage-credentials-not-offered").unwrap();
    assert_eq!(runner.run_case(case).await.outcome, Outcome::Pass);
}
