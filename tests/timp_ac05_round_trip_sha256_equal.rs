//! PRD-mcphost-export-tables-and-import-restore
//! AC5 -- Given A → B → export, When both manifests are compared, Then every
//! restorable entry's sha256 is equal except `manifest.json` and fields named
//! `created_unix`. Entries the importer reports but does not restore
//! (`runs.jsonl`, `threads.jsonl`, `usage.json`, `documents/<id>.json`) are
//! history of the source host state and legitimately differ.

use crate::common;
use crate::timp;

use std::collections::BTreeMap;

use common::TestServer;
use mcphost::export::ArchiveEntry;
use serde_json::{Value, json};

fn manifest_shas(entries: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, String> {
    let manifest: Value = serde_json::from_slice(&entries["manifest.json"]).expect("manifest json");
    manifest["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| (e["path"].as_str().expect("path").to_string(), e["sha256"].as_str().expect("sha").to_string()))
        .collect()
}

fn restorable(path: &str) -> bool {
    !matches!(
        ArchiveEntry::parse(path),
        None | Some(
            ArchiveEntry::Manifest
                | ArchiveEntry::Runs
                | ArchiveEntry::Threads
                | ArchiveEntry::Usage
                | ArchiveEntry::Document { .. }
        )
    )
}

fn without_created_unix(bytes: &[u8]) -> Value {
    let mut v: Value = serde_json::from_slice(bytes).expect("json");
    if let Some(obj) = v.as_object_mut() {
        obj.remove("created_unix");
    }
    v
}

#[tokio::test]
async fn export_of_the_import_has_the_same_sha256_per_restorable_entry() {
    let server = TestServer::start().await;
    let a = timp::new_tenant(&server, "Timp AC5 A").await;
    a.seed_full().await;
    let (run_id, a_entries) = a.export(&server).await;

    let b = timp::new_tenant(&server, "Timp AC5 B").await;
    let done = b.import_run(&run_id).await;
    assert_eq!(done["status"], json!("done"), "{done}");
    // Secret values never travel: B sets the named secret again.
    b.call("host.secret_set", json!({"name": "api_key", "value": "another"})).await;
    let (_b_run, b_entries) = b.export(&server).await;

    let (a_sha, b_sha) = (manifest_shas(&a_entries), manifest_shas(&b_entries));
    let compared: Vec<&String> = a_sha.keys().filter(|p| restorable(p)).collect();
    assert!(compared.len() >= 8, "too few restorable entries compared: {compared:?}");
    for path in compared {
        let b_hash = b_sha.get(path).unwrap_or_else(|| panic!("B's export lacks {path}"));
        if a_sha[path] == *b_hash {
            continue;
        }
        // Only a `created_unix` field may account for a difference.
        assert_eq!(
            without_created_unix(&a_entries[path]),
            without_created_unix(&b_entries[path]),
            "{path} differs beyond created_unix"
        );
    }
    assert_eq!(
        a_sha.keys().filter(|p| restorable(p)).collect::<Vec<_>>(),
        b_sha.keys().filter(|p| restorable(p)).collect::<Vec<_>>(),
        "the two archives hold different restorable entries"
    );
}
