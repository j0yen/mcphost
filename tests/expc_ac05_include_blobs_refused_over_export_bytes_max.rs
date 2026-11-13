//! PRD-mcphost-export-data-class-completeness-v2
//! AC5 -- Given a Free tenant whose `blobs` rows sum to 60 MB of `bytes`,
//! When `host.export` is called with the new optional argument
//! `include_blobs: true`, Then the call is refused `quota_exceeded
//! {export_bytes_max}` before a run row is created or any blob byte is read;
//! When `include_blobs` is absent, Then the export succeeds with
//! `blobs/index.json` only and no `blobs/<id>` entry.

use crate::common;
use crate::expc;

use common::TestServer;
use mcphost::export::EXPORT_TOOL_NAME;
use serde_json::{Value, json};

fn raw(server: &TestServer) -> rusqlite::Connection {
    rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db")
}

#[tokio::test]
async fn expc_ac05() {
    let server = TestServer::start().await;
    let tenant = expc::new_tenant(&server, "Expc AC5 Tenant").await;
    let row = server.state.db.find_tenant_by_namespace(tenant.ns.clone()).await.expect("query").expect("tenant");
    assert_eq!(row.plan, "free");
    let free = server.state.plans.get("free").expect("free plan").clone();
    assert!(free.export_bytes_max < 60_000_000, "the Free export ceiling is below 60 MB");

    tenant.put_blob("a.bin", b"first").await;
    tenant.put_blob("b.bin", b"second").await;
    // The rows claim 60 MB in total; no 60 MB file exists, so a blob byte read
    // would show as a missing file or a short entry.
    raw(&server)
        .execute("UPDATE blobs SET bytes = 30000000 WHERE tenant_id = ?1", rusqlite::params![row.id])
        .expect("size the blob rows");
    let total: i64 = raw(&server)
        .query_row("SELECT SUM(bytes) FROM blobs WHERE tenant_id = ?1", rusqlite::params![row.id], |r| r.get(0))
        .expect("sum");
    assert_eq!(total, 60_000_000);

    let runs_before: i64 = raw(&server)
        .query_row("SELECT COUNT(*) FROM runs WHERE tenant_id = ?1", rusqlite::params![row.id], |r| r.get(0))
        .expect("count");
    let err = tenant
        .client
        .tools_call("host.export", json!({"include_blobs": true}))
        .await
        .expect_err("60 MB of blobs on Free is refused");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], json!({"name": "export_bytes_max", "value": free.export_bytes_max}), "{:?}", err.data);
    assert_eq!(err.data["used"], json!(60_000_000), "{:?}", err.data);
    let runs_after: i64 = raw(&server)
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE tenant_id = ?1 AND tool_name = ?2",
            rusqlite::params![row.id, EXPORT_TOOL_NAME],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(runs_after, 0, "no run row was created (runs before: {runs_before})");

    // A wrongly typed argument is an argument error, not an export.
    let bad = tenant.client.tools_call("host.export", json!({"include_blobs": "yes"})).await.expect_err("typed");
    assert_eq!(bad.error_code.as_deref(), Some("args_invalid"), "{bad:?}");

    // Without `include_blobs`: the index only.
    let (_run, entries) = tenant.export(&server, json!({})).await;
    let index: Vec<Value> = serde_json::from_slice(&entries["blobs/index.json"]).expect("index");
    assert_eq!(index.len(), 2, "{index:?}");
    assert!(!entries.keys().any(|p| p.starts_with("blobs/") && p != "blobs/index.json"), "{:?}", entries.keys());
    let manifest: Value = serde_json::from_slice(&entries["manifest.json"]).expect("manifest");
    let blob_class = manifest["data_classes"].as_array().unwrap().iter().find(|c| c["class"] == json!("blob")).expect("blob class").clone();
    assert_eq!(blob_class["entries"], json!(0));
    assert_eq!(blob_class["sha256"], json!(mcphost::billing::sha256_hex(b"")));
}

/// With room under the ceiling, `include_blobs: true` carries the bytes.
#[tokio::test]
async fn expc_ac05_blob_bytes_when_asked() {
    let server = TestServer::start().await;
    let tenant = expc::new_tenant(&server, "Expc AC5 Bytes").await;
    tenant.put_blob("a.bin", b"first blob bytes").await;
    let (_run, entries) = tenant.export(&server, json!({"include_blobs": true})).await;
    let index: Vec<Value> = serde_json::from_slice(&entries["blobs/index.json"]).expect("index");
    assert_eq!(index.len(), 1);
    let id = index[0]["id"].as_str().expect("id");
    assert_eq!(entries[&format!("blobs/{id}")], b"first blob bytes");
    assert_eq!(index[0]["sha256"], json!(mcphost::billing::sha256_hex(b"first blob bytes")));
}
