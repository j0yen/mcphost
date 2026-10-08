//! AC2 (PRD-mcphost-plan-limits-generated) — Given tools/list, When
//! `host.tool_call.inputSchema.properties.budget` is read, Then each budget
//! field has `maximum` equal to the free-plan ceiling and a description
//! naming the pro ceiling.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::plans::{BUDGET_FIELD_PREFIX, PlanCatalog};
use serde_json::{Value, json};

#[tokio::test]
async fn tool_call_budget_schema_has_free_maximum_and_pro_description() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "PlanLim AC2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let list = client.tools_list().await.expect("tools/list ok");
    let tool = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == json!("host.tool_call"))
        .expect("host.tool_call listed");
    let budget = &tool["inputSchema"]["properties"]["budget"];

    let catalog = PlanCatalog::default_catalog();
    let budget_fields = |plan: &str| -> Vec<(String, Value)> {
        let Value::Object(f) = serde_json::to_value(catalog.get(plan).unwrap()).unwrap() else {
            panic!("object");
        };
        f.into_iter()
            .filter_map(|(k, v)| k.strip_prefix(BUDGET_FIELD_PREFIX).map(|r| (r.to_string(), v)))
            .collect()
    };
    let free = budget_fields("free");
    let pro = budget_fields("pro");
    assert!(free.len() >= 4);
    for ((field, free_max), (_, pro_max)) in free.iter().zip(pro.iter()) {
        let prop = &budget["properties"][field.as_str()];
        assert_eq!(&prop["maximum"], free_max, "maximum for {field}");
        let desc = prop["description"].as_str().expect("description");
        assert!(desc.contains(&pro_max.to_string()), "{field}: {desc} must name pro {pro_max}");
        assert!(desc.contains("host.quickstart"), "{field}: {desc}");
    }
}
