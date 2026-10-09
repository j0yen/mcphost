//! PRD-mcphost-publish-schema-from-registry
//! AC4 (P0) -- Given `host.quickstart` with no arguments, When called on a
//! connection with no tenant, Then the schema does not list `kind` under
//! `required` and the response lists every registered kind, every alias
//! and every recipe name with `try_before_call` and limits, creating no
//! tenant.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, five_kinds_registry};
use mcphost::kinds::aliases;
use serde_json::{Value, json};

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|x| x.as_str().expect("string").to_string())
        .collect()
}

#[tokio::test]
async fn quickstart_without_kind_is_an_index_and_creates_no_tenant() {
    let envs_dir = TempDataDir::new();
    let registry = five_kinds_registry(&envs_dir.0);
    let expected_kinds: Vec<String> = registry.names().into_iter().map(String::from).collect();
    let server = TestServer::start_with_kinds(registry).await;
    let client = McpClient::new(&server.base_url);

    let listed = client.tools_list().await.expect("tools/list");
    let quickstart = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == "host.quickstart")
        .expect("host.quickstart is listed anonymously");
    let required = quickstart["inputSchema"]["required"].as_array().cloned().unwrap_or_default();
    assert!(!required.contains(&json!("kind")), "kind must not be required: {required:?}");

    let result = client
        .tools_call("host.quickstart", json!({}))
        .await
        .expect("quickstart with no arguments on a tenant-less connection");
    let body = extract_structured(&result);

    assert_eq!(strings(&body["kinds"]), expected_kinds);
    assert_eq!(
        strings(&body["aliases"]),
        aliases::alias_names().into_iter().map(String::from).collect::<Vec<_>>()
    );
    assert_eq!(
        strings(&body["recipes"]),
        aliases::recipe_names().into_iter().map(String::from).collect::<Vec<_>>()
    );
    assert!(
        !body["try_before_call"].as_array().expect("try_before_call").is_empty(),
        "{body}"
    );
    assert!(body["limits"]["max_spec_bytes"].is_u64(), "limits: {}", body["limits"]);
    assert!(body["limits"]["plan"]["tools_max"].is_u64(), "plan limits: {}", body["limits"]);

    assert_eq!(server.state.db.list_tenants().await.unwrap().len(), 0, "no tenant created");
}
