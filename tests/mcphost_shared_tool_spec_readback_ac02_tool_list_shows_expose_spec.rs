//! PRD-mcphost-shared-tool-spec-readback
//! AC2 (P0) — Given the same share (a python tool shared to a group with
//! `expose_spec: true`), When the owner calls `host.tool_list`, Then the
//! tool row shows `expose_spec: true`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn owner_tool_list_shows_expose_spec_true() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (_ns_o, key_o) = signup(&server.base_url, "AC2 Owner").await;
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
            json!({"name": "nightly_scrape", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    client_o
        .tools_call(
            "host.tool_share",
            json!({
                "name": "nightly_scrape",
                "visibility": "group",
                "group": "builders",
                "expose_spec": true,
            }),
        )
        .await
        .expect("share ok");

    let listed = extract_structured(
        &client_o.tools_call("host.tool_list", json!({})).await.expect("tool_list ok"),
    );
    let tools = listed["tools"].as_array().expect("tools array");
    let row = tools
        .iter()
        .find(|t| t["name"].as_str().is_some_and(|n| n.ends_with(".nightly_scrape")))
        .expect("nightly_scrape row present");
    assert_eq!(row["expose_spec"], json!(true), "row: {row:?}");
}
