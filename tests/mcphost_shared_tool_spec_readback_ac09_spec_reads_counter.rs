//! PRD-mcphost-shared-tool-spec-readback
//! AC9 (P1) — Given three reads by sharees, When the owner lists tools,
//! Then the row shows `spec_reads: 3` and an `exposed_at` timestamp.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn three_reads_show_up_as_spec_reads_three_with_exposed_at() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (ns_o, key_o) = signup(&server.base_url, "AC9 Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call("host.group.create", json!({"name": "builders"}))
        .await
        .expect("group.create");

    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "counted", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    client_o
        .tools_call(
            "host.tool_share",
            json!({
                "name": "counted",
                "visibility": "group",
                "group": "builders",
                "expose_spec": true,
            }),
        )
        .await
        .expect("share ok");

    // Before any read at all: exposed_at is already set (from the share
    // call itself), spec_reads is still 0.
    let before = extract_structured(
        &client_o.tools_call("host.tool_list", json!({})).await.expect("tool_list ok"),
    );
    let row_before = before["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"].as_str().is_some_and(|n| n.ends_with(".counted")))
        .expect("counted row present")
        .clone();
    assert_eq!(row_before["spec_reads"], json!(0));
    assert!(!row_before["exposed_at"].is_null(), "exposed_at must already be set: {row_before:?}");

    let qualified = format!("{ns_o}.counted");
    for i in 0..3 {
        let (ns_m, key_m) = signup(&server.base_url, &format!("AC9 Member {i}")).await;
        client_o
            .tools_call("host.group.add", json!({"name": "builders", "namespace": ns_m}))
            .await
            .expect("group.add");
        let client_m = McpClient::with_bearer(&server.base_url, &key_m);
        client_m
            .tools_call("host.tool_spec_shared", json!({"tool": qualified.clone()}))
            .await
            .unwrap_or_else(|e| panic!("read {i} failed: {} {}", e.code, e.message));
    }

    let after = extract_structured(
        &client_o.tools_call("host.tool_list", json!({})).await.expect("tool_list ok"),
    );
    let row_after = after["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"].as_str().is_some_and(|n| n.ends_with(".counted")))
        .expect("counted row present");
    assert_eq!(row_after["spec_reads"], json!(3), "row: {row_after:?}");
    assert_eq!(row_after["exposed_at"], row_before["exposed_at"]);
}
