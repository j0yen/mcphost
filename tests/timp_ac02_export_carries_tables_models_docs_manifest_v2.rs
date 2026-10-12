//! PRD-mcphost-export-tables-and-import-restore
//! AC2 -- Given the same tenant after R1, When `host.export` completes, Then
//! the archive holds `tables/t/schema.json`, `tables/t/rows.jsonl` with 3
//! lines, `models/t.json`, `docs/a.md`, and `manifest.json` version 2 with a
//! `sha256` per entry.

use crate::common;
use crate::timp;

use common::TestServer;
use serde_json::{Value, json};

#[tokio::test]
async fn archive_holds_table_rows_model_doc_and_a_version_2_manifest_with_sha256() {
    let server = TestServer::start().await;
    let t = timp::new_tenant(&server, "Timp AC2 Tenant").await;
    t.seed_table(3).await;
    t.seed_doc("a.md", "# a doc").await;

    let (_run_id, entries) = t.export(&server).await;
    let listing: Vec<&String> = entries.keys().collect();

    for path in ["tables/t/schema.json", "tables/t/rows.jsonl", "models/t.json", "docs/a.md", "docs/a.md.meta.json"] {
        assert!(entries.contains_key(path), "missing {path}: {listing:?}");
    }
    let rows = String::from_utf8(entries["tables/t/rows.jsonl"].clone()).expect("utf8");
    assert_eq!(rows.lines().count(), 3, "rows.jsonl: {rows:?}");
    let first: Value = serde_json::from_str(rows.lines().next().expect("line")).expect("row json");
    assert_eq!(first, json!({"id": 1, "name": "row 1"}));
    assert_eq!(entries["docs/a.md"], b"# a doc");

    let schema: Value = serde_json::from_slice(&entries["tables/t/schema.json"]).expect("schema json");
    assert_eq!(schema["columns"], json!({"id": "integer", "name": "text"}));
    assert_eq!(schema["primary_key"], json!("id"));
    let model: Value = serde_json::from_slice(&entries["models/t.json"]).expect("model json");
    assert_eq!(model, json!([{"column": "", "key": "description", "value": "the t table"}]));

    let manifest: Value = serde_json::from_slice(&entries["manifest.json"]).expect("manifest json");
    assert_eq!(manifest["version"], json!(2), "manifest: {manifest}");
    let listed = manifest["entries"].as_array().expect("entries array");
    assert_eq!(listed.len(), entries.len() - 1, "every entry but manifest.json is listed");
    for entry in listed {
        let path = entry["path"].as_str().expect("path");
        let data = entries.get(path).unwrap_or_else(|| panic!("manifest lists {path}, archive lacks it"));
        let sha = entry["sha256"].as_str().unwrap_or_else(|| panic!("no sha256 for {path}"));
        assert_eq!(sha, mcphost::billing::sha256_hex(data), "sha256 of {path}");
        assert_eq!(entry["bytes"], json!(data.len()), "bytes of {path}");
    }
}
