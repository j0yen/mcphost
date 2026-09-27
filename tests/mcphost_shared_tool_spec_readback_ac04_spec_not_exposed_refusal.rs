//! PRD-mcphost-shared-tool-spec-readback
//! AC4 (P0) — Given a tool shared without `expose_spec`, When a sharee
//! calls `host.tool_spec_shared`, Then the response is `spec_not_exposed`
//! naming `<owner_ns>.<name>`.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn sharee_read_without_expose_spec_gets_spec_not_exposed() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (ns_o, key_o) = signup(&server.base_url, "AC4 Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call("host.group.create", json!({"name": "builders"}))
        .await
        .expect("group.create");

    let (ns_m, key_m) = signup(&server.base_url, "AC4 Member").await;
    client_o
        .tools_call("host.group.add", json!({"name": "builders", "namespace": ns_m}))
        .await
        .expect("group.add");

    let spec = json!({
        "source": "def main(args):\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "closed_spec", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    // No `expose_spec` at all -- defaults to false.
    client_o
        .tools_call(
            "host.tool_share",
            json!({"name": "closed_spec", "visibility": "group", "group": "builders"}),
        )
        .await
        .expect("share ok");

    let client_m = McpClient::with_bearer(&server.base_url, &key_m);
    let qualified = format!("{ns_o}.closed_spec");
    let err = client_m
        .tools_call("host.tool_spec_shared", json!({"tool": qualified.clone()}))
        .await
        .expect_err("must refuse: expose_spec was never set");
    assert_eq!(err.error_code.as_deref(), Some("spec_not_exposed"));
    assert!(
        err.message.contains(&qualified),
        "spec_not_exposed message must name '{qualified}': {}",
        err.message
    );
}
