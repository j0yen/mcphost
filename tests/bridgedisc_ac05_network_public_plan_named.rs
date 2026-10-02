//! PRD-mcphost-sandbox-bridge-discoverability
//! AC5 (P0) -- Given a free-plan tenant, When it calls `host.quickstart
//! kind=python`, Then `limits.plan.network_public` names the plan that
//! allows `network: "public"`, and the python row of `try_before_call`
//! says network is off by default.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn quickstart_names_the_network_public_plan_and_pythons_default() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridgedisc AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let structured = extract_structured(
        &client
            .tools_call("host.quickstart", json!({"kind": "python"}))
            .await
            .expect("quickstart kind=python"),
    );

    assert_eq!(
        structured["limits"]["plan"]["name"],
        json!("free"),
        "this is a fresh, unupgraded tenant: {structured}"
    );
    assert_eq!(
        structured["limits"]["plan"]["network_public"],
        json!("pro"),
        "limits.plan.network_public must name the plan that allows network: \"public\": {structured}"
    );

    let try_before_call = structured["try_before_call"].as_array().expect("try_before_call array");
    let python_row = try_before_call
        .iter()
        .find(|row| row["call"] == json!("host.tool_run"))
        .unwrap_or_else(|| panic!("try_before_call must carry the python (host.tool_run) row: {try_before_call:?}"));
    let note = python_row["note"].as_str().expect("the python row must carry a note");
    assert!(
        note.to_lowercase().contains("network") && note.to_lowercase().contains("off by default"),
        "the python row's note must say network is off by default: {note}"
    );
}
