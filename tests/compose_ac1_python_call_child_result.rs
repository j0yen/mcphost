//! PRD-mcphost-composition AC1 (P0) — Given tools `a` and `b` in one tenant
//! where `a`'s source calls `mcphost.call("b", {"n": 2})` and returns its
//! payload, When `a` is called with `network: none`, Then the result is
//! `b`'s payload.
//!
//! No `runs` table exists yet (PRD-mcphost-runs-and-jobs, this PRD's
//! declared dependency, hasn't shipped), so `host.runs.get(a's run)`
//! listing one child with `tool: b` can't be checked against a ledger --
//! same deferral `compose_ac3`/`compose_ac4` already document. This test
//! asserts the observable half: `a`'s result is exactly what `b` returned,
//! reached entirely over the sandbox channel (`network: none` on both
//! tools -- `mcphost.call` must work without the network `python.rs`'s own
//! sandbox blocks).

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn a_tool_calling_mcphost_call_returns_the_callees_payload() {
    // See python_ac01's own comment: this test runs real sandboxed python
    // tools, which need unprivileged user namespaces -- not guaranteed on
    // GitHub's hosted runners.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Compose AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let b_spec = json!({
        "source": "def main(args):\n    return {\"doubled\": args[\"n\"] * 2}\n",
        "args_schema": {"type": "object", "properties": {"n": {"type": "integer"}}},
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
        "source": "import mcphost\n\ndef main(args):\n    return mcphost.call(\"tool_b\", {\"n\": 2})\n",
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

    let result = poll_until_ready(
        &client,
        &format!("{ns}.tool_a"),
        json!({}),
        Duration::from_secs(15),
    )
    .await
    .unwrap_or_else(|e| panic!("a's call must succeed: {} {}", e.code, e.message));
    let structured = common::extract_structured(&result);

    assert_eq!(
        structured["doubled"], 4,
        "a's result must be exactly b's payload: {structured}"
    );
}
