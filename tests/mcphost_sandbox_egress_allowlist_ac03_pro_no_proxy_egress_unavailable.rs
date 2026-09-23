//! PRD-mcphost-sandbox-egress-allowlist
//! AC3 (P0) — Given a Pro tenant and `MCPHOST_EGRESS_PROXY` unset, When it
//! calls a tool published with `network: "egress"`, Then the run fails
//! with `egress_unavailable` and no sandbox process is spawned.

use crate::common;
use common::{ADMIN_KEY, McpClient, TempDataDir, TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

use crate::egress_proxy_lock;

#[tokio::test]
async fn pro_tenant_with_no_proxy_configured_gets_egress_unavailable() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // See egress_proxy_lock's doc comment: serializes this test against
    // the other two files in this PRD that also mutate the process-wide
    // MCPHOST_EGRESS_PROXY env var.
    let _guard = egress_proxy_lock::guard().await;
    // SAFETY: held across this whole test body via the async guard above,
    // so no other test in this binary observes a torn env var.
    unsafe {
        std::env::remove_var("MCPHOST_EGRESS_PROXY");
    }

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC3 Pro Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    admin
        .tools_call("admin.plan_set", json!({"tenant": ns, "plan": "pro", "reason": "AC3 setup"}))
        .await
        .expect("admin.plan_set to pro");

    let spec = json!({
        "source": "import socket\ndef main(args):\n    s = socket.create_connection(('1.1.1.1', 80), timeout=3)\n    s.close()\n    return {\"connected\": True}\n",
        "args_schema": {"type": "object"},
        "network": "egress",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "dialer", "kind": "python", "spec": spec}),
        )
        .await
        .expect("pro tenant publish of network: egress must succeed");

    let err = client
        .tools_call("host.tool_call", json!({"name": "dialer", "args": {}}))
        .await
        .expect_err("no proxy configured must refuse the call outright");

    assert_eq!(err.error_code.as_deref(), Some("egress_unavailable"));
}
