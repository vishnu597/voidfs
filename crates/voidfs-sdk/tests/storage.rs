// SPDX-License-Identifier: Apache-2.0
//! Reading a drive's storage with storage credentials (protocol §5.5), against a server in this
//! process and the stand-in bucket that mints and enforces them.

mod common;

use common::client_for;
use voidfs_sdk::{Client, Config};
use voidfs_server::sigv4::{KeyInfo, Scope};
use voidfs_server::test_server::{Rules, TestServer};

const READ_ID: &str = "VFTESTREADKEY2345678";
const READ_SECRET: &str = "readsecretreadsecretreadsecretreadsecret";

#[tokio::test(flavor = "multi_thread")]
async fn storage_reads_what_its_credentials_reach() {
    let read = KeyInfo { id: READ_ID.into(), secret: READ_SECRET.into(), scope: Scope::Read, drives: Some(vec!["drv".into()]) };
    // Listings in pages of 2, which the reader follows.
    let s = TestServer::with_storage_credentials(vec![read], Rules { list_page: 2, ..Rules::default() }).await.unwrap();
    let admin = client_for(&s.endpoint, Config::default());
    admin.create_drive("drv", Default::default()).await.unwrap();
    admin.create_drive("other", Default::default()).await.unwrap();
    for i in 1..=3 {
        admin.put_object("drv", &format!("f{i}"), vec![i as u8; 5000], Default::default()).await.unwrap();
    }
    let reader = Client::new(Config { endpoint: s.endpoint.clone(), access_key_id: READ_ID.into(), secret_access_key: READ_SECRET.into(), ..Config::default() }).unwrap();
    let creds = reader.storage_credentials("drv").await.unwrap();
    let id = creds.drive_id.clone();
    let st = reader.storage(creds).unwrap();
    assert!(st.get("voidfs.json").await.unwrap().is_some());
    let drive: serde_json::Value = serde_json::from_slice(&st.get(&format!("drives/{id}/drive.json")).await.unwrap().unwrap()).unwrap();
    assert_eq!(drive["drive_id"], id.as_str());
    assert_eq!(st.get(&format!("drives/{id}/nothing.json")).await.unwrap(), None, "a missing object is None");
    let log = st.list(&format!("drives/{id}/log/"), None).await.unwrap();
    assert_eq!(log, ["00000000000000000001.json", "00000000000000000002.json", "00000000000000000003.json"]);
    assert_eq!(st.list(&format!("drives/{id}/log/"), Some("00000000000000000001.json")).await.unwrap(), &log[1..], "after a name");
    assert_eq!(st.list(&format!("drives/{id}/checkpoints/"), None).await.unwrap(), Vec::<String>::new());
    // What they don't reach is refused, not missing.
    let other = s.pool.drive("other").unwrap().id.clone();
    assert_eq!(st.get(&format!("drives/{other}/drive.json")).await.unwrap_err().status(), Some(403));
    assert_eq!(st.list("drives/", None).await.unwrap_err().status(), Some(403));
}
