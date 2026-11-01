//! PRD-mcphost-chain-near-miss-mapping-refused AC4 (P0) -- Given the AC 1
//! chain, When published with `force: true`, Then the tool exists, the
//! response `gates` entry reads `verdict: will_fail, forced: true`, and a call
//! of it passes the object through to `transform_data` as a literal, observed
//! as the AC 6 error `step 2 ('transform_data'): TypeError ...` (same fixture
//! as AC 6).

use crate::common;
use common::{McpClient, TempDataDir, TestServer, chain_and_python_kind_registry, extract_structured, publish, signup};
use mcphost::sandbox;
use serde_json::json;

const FETCH: &str = "def main(args):\n    return {\"rows\": [{\"id\": 1, \"value\": 2}]}\n";
const TRANSFORM: &str = "def main(args):\n    return {\"total\": sum(r[\"value\"] for r in args[\"rows\"])}\n";

#[tokio::test]
async fn a_forced_publish_keeps_the_object_as_a_literal_and_the_call_names_its_step() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(chain_and_python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Chainmap AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "fetch_data", "python", json!({"source": FETCH})).await;
    publish(&client, "transform_data", "python", json!({"source": TRANSFORM})).await;

    let spec = json!({"steps": [
        {"tool": "fetch_data", "args": {}},
        {"tool": "transform_data", "args": {"rows": {"from": "steps.0.rows"}}},
    ]});
    let published = extract_structured(
        &client
            .tools_call(
                "host.tool_publish",
                json!({"name": "daily_pipeline", "kind": "chain", "spec": spec, "force": true}),
            )
            .await
            .expect("force: true must publish the flagged chain"),
    );
    assert_eq!(published["name"], json!(format!("{ns}.daily_pipeline")), "{published}");
    let gate = published["gates"]
        .as_array()
        .and_then(|g| g.first())
        .unwrap_or_else(|| panic!("a forced publish carries a gates entry: {published}"));
    assert_eq!(gate["verdict"], json!("will_fail"), "{published}");
    assert_eq!(gate["forced"], json!(true), "{published}");

    let list = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("list"));
    let names: Vec<&str> =
        list["tools"].as_array().expect("tools array").iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&format!("{ns}.daily_pipeline").as_str()), "the forced tool must exist: {names:?}");

    // The object reaches `transform_data` unchanged: iterating it yields the
    // key "from", and `"from"["value"]` is the TypeError.
    let err = client
        .tools_call("host.tool_call", json!({"name": "daily_pipeline", "args": {}}))
        .await
        .expect_err("the literal object breaks transform_data");
    assert!(err.message.starts_with("step 2 ('transform_data'): "), "{err:?}");
    assert!(err.message.contains("TypeError"), "{err:?}");
    assert_eq!(err.data["failed_step"], json!(2), "{:?}", err.data);
    assert_eq!(err.data["step_tool"], json!("transform_data"), "{:?}", err.data);
}
