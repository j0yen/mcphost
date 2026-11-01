//! PRD-mcphost-chain-near-miss-mapping-refused AC1 (P0) -- Given python tools
//! `fetch_data` and `transform_data` with no declared `outputs` and no
//! completed run, When a chain whose step 2 maps `"rows": {"from":
//! "steps.0.rows"}` is published without `force`, Then the reply is
//! `compose_mapping_unresolved`, `data.step` is 2, `data.reason` is `not a
//! path`, `data.did_you_mean` is `$.steps[0].result.rows`, the message is
//! `step 2 reads steps.0.rows: not a path`, and no tool row exists.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, chain_and_python_kind_registry, extract_structured, publish, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

const FETCH: &str = "def main(args):\n    return {\"rows\": [{\"id\": 1, \"value\": 2}]}\n";
const TRANSFORM: &str = "def main(args):\n    return {\"total\": sum(r[\"value\"] for r in args[\"rows\"])}\n";

fn chain_spec() -> Value {
    json!({"steps": [
        {"tool": "fetch_data", "args": {}},
        {"tool": "transform_data", "args": {"rows": {"from": "steps.0.rows"}}},
    ]})
}

#[tokio::test]
async fn an_object_mapping_in_the_other_dialect_is_refused_with_the_canonical_path() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(chain_and_python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Chainmap AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "fetch_data", "python", json!({"source": FETCH})).await;
    publish(&client, "transform_data", "python", json!({"source": TRANSFORM})).await;

    let err = client
        .tools_call("host.tool_publish", json!({"name": "daily_pipeline", "kind": "chain", "spec": chain_spec()}))
        .await
        .expect_err("a reference in another dialect must be refused at publish");

    assert_eq!(err.error_code.as_deref(), Some("compose_mapping_unresolved"), "{err:?}");
    assert_eq!(err.data["step"], json!(2), "{:?}", err.data);
    assert_eq!(err.data["reason"], json!("not a path"), "{:?}", err.data);
    assert_eq!(err.data["path"], json!("steps.0.rows"), "{:?}", err.data);
    assert_eq!(err.data["did_you_mean"], json!("$.steps[0].result.rows"), "{:?}", err.data);
    assert_eq!(err.message, "step 2 reads steps.0.rows: not a path", "{err:?}");

    let list = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("list"));
    let names: Vec<&str> =
        list["tools"].as_array().expect("tools array").iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(!names.contains(&format!("{ns}.daily_pipeline").as_str()), "nothing must be published: {names:?}");
}
