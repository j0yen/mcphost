//! PRD-mcphost-refusals-name-the-nearest-move
//! AC1 (P0) -- Given a tenant with tools `tide_report` and `tide_summary`,
//! When it calls `tide_reprot` with args `{"port": "sfo"}`, Then the
//! `tool_not_found` data has `did_you_mean[0]` `tide_report`, `nearest` led by
//! its two tools, `tools_published` `2`, `next_call` `{tool: "host.tool_call",
//! args: {name: "tide_report", args: {port: "sfo"}}}`, and one
//! `record_did_you_mean` row is written.

use crate::common::{McpClient, TestServer, http_kind_registry, publish, signup};
use serde_json::json;

#[tokio::test]
async fn nmove_ac01() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Nmove01").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({"method": "GET", "url": "https://api.example.com/v1/things"});
    publish(&client, "tide_report", "http", spec.clone()).await;
    publish(&client, "tide_summary", "http", spec).await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    assert!(server.state.db.did_you_mean_outcomes(tenant.id).await.unwrap().is_empty());

    let err = client
        .tools_call("tide_reprot", json!({"port": "sfo"}))
        .await
        .expect_err("tide_reprot is not a tool");

    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"), "{err:?}");
    assert_eq!(err.message, "tool not found: tide_reprot");
    let d = &err.data;
    assert_eq!(d["did_you_mean"][0], "tide_report", "{d}");
    assert_eq!(d["nearest"][0], "tide_report", "{d}");
    assert_eq!(d["nearest"][1], "tide_summary", "{d}");
    assert!(d["nearest"].as_array().unwrap().len() <= 5);
    assert_eq!(d["tools_published"], 2);
    assert_eq!(d["catalog_matches"], 0);
    assert_eq!(
        d["next_call"],
        json!({"tool": "host.tool_call", "args": {"name": "tide_report", "args": {"port": "sfo"}}})
    );
    assert_eq!(server.state.db.did_you_mean_outcomes(tenant.id).await.unwrap().len(), 1);
}
