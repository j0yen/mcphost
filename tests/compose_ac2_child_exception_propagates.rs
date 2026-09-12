//! PRD-mcphost-composition AC2 (P0) — Given `b` raising an exception, When
//! `a` calls it (via `mcphost.call`), Then `a` receives a structured
//! exception with `error_code: tool_exception` (and the child run reads
//! `error` -- deferred, see `compose_ac1`'s own note: no `runs` table
//! exists yet).
//!
//! `a` never catches the `mcphost.CallError` `mcphost.call` raises on a
//! failed child, so it escapes `a`'s own `main(args)` uncaught -- the same
//! path any other unhandled python exception takes
//! (`kinds::python::map_envelope_line`'s exception branch), which is what
//! AC2 exercises: the *caller*'s own call ends `tool_exception`, carrying
//! the child's failure in its message.

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn an_uncaught_child_exception_surfaces_as_tool_exception_on_the_caller() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Compose AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let b_spec = json!({
        "source": "def main(args):\n    raise ValueError(\"b blew up\")\n",
        "args_schema": {"type": "object"},
        "network": "none",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "tool_b", "kind": "python", "spec": b_spec}),
        )
        .await
        .expect("publish b ok");

    let a_spec = json!({
        "source": "import mcphost\n\ndef main(args):\n    return mcphost.call(\"tool_b\", {})\n",
        "args_schema": {"type": "object"},
        "network": "none",
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "tool_a", "kind": "python", "spec": a_spec}),
        )
        .await
        .expect("publish a ok");

    let err = poll_until_ready(
        &client,
        &format!("{ns}.tool_a"),
        json!({}),
        Duration::from_secs(15),
    )
    .await
    .expect_err("a's call must fail once b raises");

    assert_eq!(
        err.error_code.as_deref(),
        Some("tool_exception"),
        "a's own call must end tool_exception, not b's raw error: {err:?}"
    );
    assert!(
        err.message.contains("b blew up") || err.message.contains("McphostCallError"),
        "a's error message should carry b's failure through: {err:?}"
    );
}
