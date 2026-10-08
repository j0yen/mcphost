//! AC5 (PRD-mcphost-plan-limits-generated) — Given `host.tool_publish`'s
//! schema, When the `network` field description is read, Then it lists the
//! plan-gated values from the plan table.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::network_policy::{NETWORK_VALUES, wants_egress};
use serde_json::json;

#[tokio::test]
async fn publish_schema_network_description_lists_plan_gated_values() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "PlanLim AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let list = client.tools_list().await.expect("tools/list ok");
    let publish = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == json!("host.tool.publish") || t["name"] == json!("host.tool_publish"))
        .expect("publish tool listed");
    let desc = publish["inputSchema"]["properties"]["spec"]["description"]
        .as_str()
        .expect("spec description");
    for (value, plan) in NETWORK_VALUES {
        assert!(desc.contains(&format!("\"{value}\"")), "{desc} lacks {value}");
        if wants_egress(Some(value)) {
            assert!(
                desc.contains(&format!("\"{value}\" (requires the {plan} plan)")),
                "{desc} must say {value} requires {plan}"
            );
        }
    }
}
