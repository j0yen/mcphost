//! PRD-mcphost-blob-retention-and-quota
//! AC6 -- Given free disk below `DEFAULT_DISK_FLOOR_MB`, When
//! `host.blob.put` runs, Then it is refused with the same `disk_floor` code
//! SQL writes use and no file is written.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use base64::Engine as _;
use mcphost::retention::DEFAULT_DISK_FLOOR_MB;
use serde_json::json;

fn put_args(name: &str) -> serde_json::Value {
    json!({"name": name, "content_base64": base64::engine::general_purpose::STANDARD.encode(b"payload")})
}

#[tokio::test]
async fn put_below_the_disk_floor_is_refused_and_writes_nothing() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Blobgc AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().unwrap();

    // Just under the default floor (the test server reads no env override).
    assert_eq!(server.state.disk_guard.floor_bytes(), DEFAULT_DISK_FLOOR_MB * 1024 * 1024);
    server.state.disk_guard.set_free_bytes_override_for_test(Some(DEFAULT_DISK_FLOOR_MB * 1024 * 1024 - 1));

    let err = client.tools_call("host.blob.put", put_args("refused.txt")).await.expect_err("below the floor");
    // The same refusal a SQL write gets (mcphost_data_retention_ac06).
    assert_eq!(err.error_code.as_deref(), Some("service_unavailable"), "{err:?}");
    assert_eq!(err.data["reason"], json!("disk_floor"), "{:?}", err.data);

    assert_eq!(server.state.db.blobs_usage(tenant.id).await.unwrap(), (0, 0), "no row");
    let tenant_dir = server.state.db.data_dir().join("blobs").join(tenant.id.to_string());
    let files = std::fs::read_dir(&tenant_dir).map(|d| d.count()).unwrap_or(0);
    assert_eq!(files, 0, "no file (not even a .part) under {tenant_dir:?}");

    // Back above the floor the same put goes through.
    server.state.disk_guard.set_free_bytes_override_for_test(None);
    let ok = extract_structured(&client.tools_call("host.blob.put", put_args("refused.txt")).await.expect("put"));
    assert_eq!(ok["name"], json!("refused.txt"));
    assert_eq!(std::fs::read_dir(&tenant_dir).unwrap().count(), 1);
}
