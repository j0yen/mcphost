//! PRD-mcphost-fields-projection-on-list-verbs
//! AC2 (P0) -- Given a tenant with 200 tools, When
//! `host.tool.list(fields=["name","kind"], limit=200)` is called, Then every
//! row has exactly those two keys and the response is < 12 KiB.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn tool_list_projects_200_tools_to_name_and_kind_under_12_kib() {
    let server = TestServer::start().await;
    let (ns, key) = common::signup(&server.base_url, "Fldp AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // `host.tool.publish` stops at MAX_TOOLS_PER_TENANT (50), so the 200-tool
    // tenant is seeded through the same db write publish ends in.
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
    for i in 0..200 {
        server
            .state
            .db
            .upsert_tool(
                tenant.id,
                format!("t{i:03}"),
                "echo".to_string(),
                json!({"schema": {"type": "object"}}),
                10,
                Vec::new(),
            )
            .await
            .unwrap_or_else(|e| panic!("seed t{i:03}: {e}"));
    }

    let full = extract_structured(&client.tools_call("host.tool.list", json!({})).await.expect("list"));
    assert_eq!(full["tools"].as_array().expect("tools").len(), 200);
    assert!(full["tools"][0].as_object().expect("row").len() > 2, "an unprojected row carries more keys");

    let result = client
        .tools_call("host.tool.list", json!({"fields": ["name", "kind"], "limit": 200}))
        .await
        .expect("projected list");
    let page = extract_structured(&result);
    let tools = page["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 200);
    for row in tools {
        let mut keys: Vec<&str> = row.as_object().expect("row").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["kind", "name"], "{row}");
    }
    let bytes = serde_json::to_vec(&page).expect("serialize").len();
    assert!(bytes < 12 * 1024, "projected response is {bytes} bytes, want < 12288");
    assert!(page.get("truncated").is_none(), "limit 200 covers all 200 tools: {page}");

    let first_ten = extract_structured(
        &client.tools_call("host.tool.list", json!({"fields": ["name"], "limit": 10})).await.expect("limit 10"),
    );
    assert_eq!(first_ten["tools"].as_array().expect("tools").len(), 10);
    assert_eq!(first_ten["truncated"], json!(true));
}
