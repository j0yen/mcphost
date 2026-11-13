//! PRD-mcphost-export-data-class-completeness-v2
//! AC3 -- Given a tenant with 2 threads of 3 messages, 4 calls, 1 secret and
//! 2 blobs, When `host.export` completes, Then `threads.jsonl` holds the 6
//! message rows, `secrets.txt` holds the name, no entry's bytes contain the
//! secret's value, `calls.jsonl` has 4 lines, `blobs/index.json` lists 2 ids,
//! and `manifest.json.data_classes` has one row per class with `sha256` over
//! that class's entries in path order.

use crate::common;
use crate::expc;

use common::TestServer;
use mcphost::export::{ArchiveEntry, EXPORT_EXCLUSIONS};
use serde_json::{Value, json};

#[tokio::test]
async fn expc_ac03() {
    let server = TestServer::start().await;
    let tenant = expc::new_tenant(&server, "Expc AC3 Tenant").await;
    let sender = expc::new_tenant(&server, "Expc AC3 Sender").await;

    tenant.receive_threads(&sender, 2, 3).await;
    tenant.make_calls(4).await;
    tenant.call("host.secret_set", json!({"name": "api_key", "value": expc::SECRET_VALUE})).await;
    tenant.put_blob("one.bin", b"first blob").await;
    tenant.put_blob("two.bin", b"second blob").await;

    let (_run, entries) = tenant.export(&server, json!({})).await;

    assert_eq!(expc::lines(&entries["threads.jsonl"]).len(), 6, "2 threads x 3 messages");
    assert_eq!(String::from_utf8_lossy(&entries["secrets.txt"]).trim(), "api_key");
    for (path, data) in &entries {
        assert!(
            !String::from_utf8_lossy(data).contains(expc::SECRET_VALUE),
            "{path} holds the secret's value"
        );
    }
    let calls = expc::lines(&entries["calls.jsonl"]);
    assert_eq!(calls.len(), 4, "{calls:?}");
    let blob_index: Vec<Value> = serde_json::from_slice(&entries["blobs/index.json"]).expect("blob index json");
    assert_eq!(blob_index.len(), 2, "{blob_index:?}");
    for row in &blob_index {
        for key in ["id", "name", "version", "mime", "bytes", "sha256"] {
            assert!(row.get(key).is_some(), "blob index row lacks {key}: {row}");
        }
    }
    assert!(!entries.keys().any(|p| p.starts_with("blobs/") && p != "blobs/index.json"), "no blob bytes by default");

    // The manifest: one data-class row per class, each sha256 re-derived from
    // the archive entries in path order.
    let manifest: Value = serde_json::from_slice(&entries["manifest.json"]).expect("manifest json");
    assert_eq!(manifest["version"], json!(2));
    assert!(manifest["exported_unix"].as_i64().is_some_and(|t| t > 0), "{manifest}");
    assert_eq!(manifest["host_version"], json!(env!("CARGO_PKG_VERSION")));
    let classes = manifest["data_classes"].as_array().expect("data_classes");
    let expected: Vec<&str> = ArchiveEntry::classes().iter().map(|e| e.class_name()).collect();
    let got: Vec<&str> = classes.iter().map(|c| c["class"].as_str().expect("class")).collect();
    assert_eq!(got, expected, "one row per class, in layout order");
    for class in classes {
        let name = class["class"].as_str().unwrap();
        let mut paths: Vec<&String> = entries
            .keys()
            .filter(|p| ArchiveEntry::parse(p).is_some_and(|e| e.class_name() == name))
            .collect();
        paths.sort();
        let bytes: Vec<u8> = paths.iter().flat_map(|p| entries[*p].iter().copied()).collect();
        assert_eq!(class["entries"], json!(paths.len()), "{name} entries");
        assert_eq!(class["sha256"], json!(mcphost::billing::sha256_hex(&bytes)), "{name} sha256");
    }
    let by_class = |name: &str| classes.iter().find(|c| c["class"] == json!(name)).expect(name).clone();
    assert_eq!(by_class("threads")["rows"], json!(6));
    assert_eq!(by_class("calls")["rows"], json!(4));
    assert_eq!(by_class("blob_index")["rows"], json!(2));
    assert_eq!(by_class("secrets")["rows"], json!(1));
    // An empty class is still a row, with the sha256 of zero bytes.
    let empty = by_class("oauth_audit");
    assert_eq!(empty["entries"], json!(1));
    assert_eq!(empty["rows"], json!(0));
    assert_eq!(empty["sha256"], json!(mcphost::billing::sha256_hex(b"")));

    // `excluded` is rendered from EXPORT_EXCLUSIONS and names only tables no class covers.
    let excluded = manifest["excluded"].as_array().expect("excluded");
    assert_eq!(excluded.len(), EXPORT_EXCLUSIONS.len());
    let covered = mcphost::export::covered_tables();
    for row in excluded {
        let table = row["table"].as_str().expect("table");
        assert!(!covered.contains(&table), "{table} is both a class source and excluded");
        assert!(row["reason"].as_str().is_some_and(|r| !r.is_empty()), "{row}");
    }
    assert!(excluded.iter().any(|r| r["table"] == json!("meter_events")), "meter_events is excluded");
}
