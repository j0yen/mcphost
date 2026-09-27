//! PRD-mcphost-shared-tool-spec-readback
//! AC6 (P0) — Given a share with `expose_spec: true` that the owner then
//! unshares, When the former sharee calls `host.tool_spec_shared`, Then
//! not-found; and after re-sharing without the flag, `spec_not_exposed`.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn unshare_then_reshare_without_flag_walks_not_found_to_spec_not_exposed() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (ns_o, key_o) = signup(&server.base_url, "AC6 Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call("host.group.create", json!({"name": "builders"}))
        .await
        .expect("group.create");

    let (ns_m, key_m) = signup(&server.base_url, "AC6 Member").await;
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
            json!({"name": "revocable", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    client_o
        .tools_call(
            "host.tool_share",
            json!({
                "name": "revocable",
                "visibility": "group",
                "group": "builders",
                "expose_spec": true,
            }),
        )
        .await
        .expect("share with expose_spec ok");

    let client_m = McpClient::with_bearer(&server.base_url, &key_m);
    let qualified = format!("{ns_o}.revocable");

    // Sanity: the read works before any unshare.
    client_m
        .tools_call("host.tool_spec_shared", json!({"tool": qualified.clone()}))
        .await
        .expect("read works while shared with expose_spec: true");

    client_o
        .tools_call("host.tool_unshare", json!({"name": "revocable"}))
        .await
        .expect("unshare ok");

    let err = client_m
        .tools_call("host.tool_spec_shared", json!({"tool": qualified.clone()}))
        .await
        .expect_err("a former sharee must be refused after unshare");
    assert_eq!(err.error_code.as_deref(), Some("tool_not_found"));

    // Re-share, this time omitting expose_spec -- back to closed, not
    // reopened at its old value.
    client_o
        .tools_call(
            "host.tool_share",
            json!({"name": "revocable", "visibility": "group", "group": "builders"}),
        )
        .await
        .expect("re-share without expose_spec ok");

    let err = client_m
        .tools_call("host.tool_spec_shared", json!({"tool": qualified}))
        .await
        .expect_err("re-shared without expose_spec must refuse as spec_not_exposed");
    assert_eq!(err.error_code.as_deref(), Some("spec_not_exposed"));
}
