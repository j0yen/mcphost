//! PRD-mcphost-shared-tool-spec-readback
//! AC1 (P0) — Given an owner publishes a python tool and shares it to a
//! group with `expose_spec: true`, When a group member calls
//! `host.tool_spec_shared`, Then the response has `kind: "python"` and a
//! `spec` containing `source` and `args_schema`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn group_member_reads_kind_and_spec_after_expose_spec_share() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (ns_o, key_o) = signup(&server.base_url, "AC1 Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call("host.group.create", json!({"name": "builders"}))
        .await
        .expect("group.create");

    let (ns_m, key_m) = signup(&server.base_url, "AC1 Member").await;
    client_o
        .tools_call("host.group.add", json!({"name": "builders", "namespace": ns_m}))
        .await
        .expect("group.add");

    let source = "def main(args):\n    return {\"ok\": True}\n";
    let spec = json!({
        "source": source,
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

    let client_m = McpClient::with_bearer(&server.base_url, &key_m);
    let result = client_m
        .tools_call(
            "host.tool_spec_shared",
            json!({"tool": format!("{ns_o}.nightly_scrape")}),
        )
        .await
        .expect("tool_spec_shared ok");
    let structured = extract_structured(&result);

    assert_eq!(structured["kind"], json!("python"));
    assert_eq!(structured["spec"]["source"], json!(source));
    assert_eq!(structured["spec"]["args_schema"], json!({"type": "object"}));
}
