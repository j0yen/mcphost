//! PRD-mcphost-sync-source-registry-and-create AC4 (P0) -- Given `kind:
//! "excel_online"`, a `csv_url` ref of `http://10.0.0.1/x.csv`, and `kind:
//! "google_sheet"` on a tenant with no Google connection, When each is
//! created, Then the errors are `args_invalid` (message lists the five
//! kinds), `host_not_allowed`, and `connection_missing {provider: "google"}`,
//! and no `syncs` row exists.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::sync::SourceKind;
use serde_json::json;

#[tokio::test]
async fn syreg_ac04() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Syreg AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let unknown = client
        .tools_call("host.sync.create", json!({"source": {"kind": "excel_online", "ref": "book-1"}, "table": "orders"}))
        .await
        .expect_err("an unknown kind is refused");
    assert_eq!(unknown.error_code.as_deref(), Some("args_invalid"), "{unknown:?}");
    assert_eq!(SourceKind::ALL.len(), 5);
    for kind in SourceKind::ALL {
        assert!(unknown.message.contains(kind.as_str()), "the message lists {}: {}", kind.as_str(), unknown.message);
    }

    let private = client
        .tools_call("host.sync.create", json!({"source": {"kind": "csv_url", "ref": "http://10.0.0.1/x.csv"}, "table": "orders"}))
        .await
        .expect_err("a private literal host is refused");
    assert_eq!(private.error_code.as_deref(), Some("host_not_allowed"), "{private:?}");

    let unconnected = client
        .tools_call("host.sync.create", json!({"source": {"kind": "google_sheet", "ref": "sheet-1"}, "table": "orders"}))
        .await
        .expect_err("google_sheet needs a google connection");
    assert_eq!(unconnected.error_code.as_deref(), Some("connection_missing"), "{unconnected:?}");
    assert_eq!(unconnected.data["provider"], "google", "{unconnected:?}");
    assert!(unconnected.data["hint"].as_str().unwrap_or_default().contains("host.connect.start"), "{unconnected:?}");

    // Nothing was stored: no syncs row, and no table was created on the way.
    let listed = extract_structured(&client.tools_call("host.sync.list", json!({})).await.expect("list"));
    assert_eq!(listed["syncs"], json!([]), "{listed}");
    let tables = extract_structured(&client.tools_call("host.table.list", json!({})).await.expect("table.list"));
    assert!(!tables.to_string().contains("orders"), "{tables}");
}
