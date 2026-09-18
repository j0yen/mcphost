//! PRD-mcphost-tool-run-envelope AC3 — Given the descriptors and llms.txt
//! after build, When read, Then `tool_run` documents the standard envelope
//! plus run metadata and the dry-run decision table reflects it.

use crate::common;
use common::{TempDataDir, TestServer, all_kinds_registry, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn tool_run_descriptor_and_decision_table_name_the_envelope() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Runenvelope AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // `tools/list`'s own `host.tool_run` descriptor names the envelope
    // (`result.payload`) alongside its run metadata -- not just "stdout,
    // stderr and exit code" as before this PRD.
    let listed = client.tools_list().await.expect("tools/list");
    let tools = listed["tools"].as_array().expect("tools array");
    let tool_run = tools
        .iter()
        .find(|t| t["name"] == json!("host.tool_run"))
        .expect("host.tool_run must be listed");
    let description = tool_run["description"].as_str().expect("description string");
    assert!(
        description.contains("payload"),
        "host.tool_run's descriptor must name the result envelope: {description}"
    );
    assert!(
        description.contains("host.quickstart"),
        "host.tool_run's descriptor must still point at host.quickstart: {description}"
    );
    assert!(
        description.len() <= 160,
        "host.tool_run's descriptor must stay within surface_ac02's 160-char budget, got {}: \
         {description}",
        description.len()
    );

    // `host.quickstart`'s try_before_call decision table row for
    // `host.tool_run` names the envelope too, not just the raw debug
    // fields.
    let quickstart = client
        .tools_call("host.quickstart", json!({"kind": "python"}))
        .await
        .expect("quickstart");
    let structured = extract_structured(&quickstart);
    let rows = structured["try_before_call"]
        .as_array()
        .expect("try_before_call must be an array");
    let tool_run_row = rows
        .iter()
        .find(|r| r["call"] == json!("host.tool_run"))
        .expect("try_before_call must name host.tool_run");
    let case = tool_run_row["case"].as_str().expect("case string");
    assert!(
        case.contains("payload"),
        "host.tool_run's decision-table case must name the result envelope: {case}"
    );
}
