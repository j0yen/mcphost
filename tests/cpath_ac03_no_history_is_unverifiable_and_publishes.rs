//! PRD-mcphost-chain-publish-time-path-check AC3 (P0) -- Given a predecessor
//! with no declared `outputs` and no completed run, When the chain is
//! published, Then the verdict is `unverifiable`, the tool is published, and
//! `next` names `host.tool_test`.

use crate::common;
use common::{McpClient, TestServer, chain_and_http_kind_registry, extract_structured, publish, signup};
use serde_json::json;

async fn setup(name: &str) -> (TestServer, McpClient, String) {
    let server = TestServer::start_with_kinds(chain_and_http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, name).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    (server, client, ns)
}

#[tokio::test]
async fn a_predecessor_that_never_ran_stays_unverifiable() {
    let (_server, client, ns) = setup("Cpath AC3 Tenant").await;
    publish(&client, "src", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "sink", "echo", json!({"schema": {"type": "object"}})).await;

    let spec = json!({"steps": [
        {"tool": "src", "args": {}},
        {"tool": "sink", "args": {"rows": "$.prev.result.rows"}},
    ]});
    let published = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "pipeline", "kind": "chain", "spec": spec}))
            .await
            .expect("an unverifiable chain must still publish"),
    );

    assert_eq!(published["verdict"], json!("unverifiable"), "{published}");
    assert_eq!(published["next"]["tool"], json!("host.tool_test"), "{published}");
    assert_eq!(published["name"], json!(format!("{ns}.pipeline")), "{published}");

    let list = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("list"));
    let names: Vec<&str> = list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&format!("{ns}.pipeline").as_str()), "the chain must be published: {names:?}");
}
