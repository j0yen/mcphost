//! PRD-mcphost-python-dependency-policy
//! AC4 (P0) — Given a `free` tenant, When a python tool is published
//! without `network`, Then it runs with `network: none` and an outbound
//! connect from it fails.

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn free_tenant_publish_with_no_network_field_blocks_outbound_connect() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    // `signup` always lands a fresh tenant on the `free` plan (`db.rs`'s own
    // `plan: "free".to_string()` default) -- no explicit plan-set call
    // needed to exercise this AC.
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // No `network` key at all -- the exact scenario this AC names.
    let spec = json!({
        "source": "import socket\ndef main(args):\n    s = socket.create_connection(('1.1.1.1', 80), timeout=3)\n    s.close()\n    return {\"connected\": True}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "dialer", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = poll_until_ready(
        &client,
        &format!("{ns}.dialer"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .expect_err("a free tenant's default network:none must block an outbound connect");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"));
}
