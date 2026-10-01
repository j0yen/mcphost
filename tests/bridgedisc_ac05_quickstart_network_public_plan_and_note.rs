//! PRD-mcphost-sandbox-bridge-discoverability
//! AC5 (P0) -- Given a free-plan tenant, When it calls `host.quickstart
//! kind=python`, Then `limits.plan.network_public` names the plan that
//! allows `network: "public"`, and the python row of `try_before_call`
//! says network is off by default.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use serde_json::json;

#[tokio::test]
async fn quickstart_names_network_public_plan_and_default() {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Bridge AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.quickstart", json!({"kind": "python"}))
        .await
        .expect("quickstart kind=python");
    let structured = extract_structured(&result);

    // A fresh signup is free-plan by default -- `network: "public"` is
    // refused for it (PRD-mcphost-sandbox-egress-allowlist), so
    // `network_public` must name a *different* plan than this tenant's own.
    let plan = &structured["limits"]["plan"];
    assert_eq!(plan["name"], json!("free"), "a fresh tenant must be on the free plan: {plan}");
    let network_public = plan["network_public"]
        .as_str()
        .unwrap_or_else(|| panic!("limits.plan.network_public missing: {structured}"));
    assert_ne!(
        network_public, "free",
        "network_public must name the plan that LIFTS the gate, not the one still under it"
    );

    let try_before_call = structured["try_before_call"]
        .as_array()
        .expect("try_before_call array");
    let python_row = try_before_call
        .iter()
        .find(|row| row["call"] == json!("host.tool_run"))
        .unwrap_or_else(|| panic!("try_before_call must have a host.tool_run row: {try_before_call:?}"));
    let note = python_row["note"]
        .as_str()
        .unwrap_or_else(|| panic!("host.tool_run row must carry a note: {python_row}"));
    assert!(
        note.to_lowercase().contains("network") && note.to_lowercase().contains("default"),
        "the python row's note must say network is off by default: {note}"
    );
}
