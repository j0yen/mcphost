//! AC12 (P0) — Given a republish of the same name while a call is in
//! flight, When both complete, Then the in-flight call returns the old
//! source's result and the next call returns the new source's result.
//!
//! `handler.rs::call_published_tool` reads the tool's row (spec included)
//! from the database once, at the start of dispatch, before handing it to
//! `Kind::call`; a republish's `UPDATE` only changes what the *next* read
//! sees. This test proves that behaviorally: it starts a slow call against
//! v1, republishes to v2 while v1 is still sleeping, then checks both the
//! in-flight and the subsequent call land on the source that was current
//! when *they* started.

mod common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn republish_is_atomic_for_an_in_flight_call() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC12 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let v1 = json!({
        "source": "import time\ndef main(args):\n    time.sleep(1)\n    return {\"version\": 1}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "versioned", "kind": "python", "spec": v1}),
        )
        .await
        .expect("publish v1 ok");

    // Warm the (no-dependency) build so the timed in-flight call below
    // measures the real sleep, not a `tool_building` response.
    let _ = poll_until_ready(
        &client,
        &format!("{ns}.versioned"),
        json!({}),
        Duration::from_secs(5),
    )
    .await;

    let in_flight_client = common::McpClient::with_bearer(&server.base_url, &key);
    let qualified = format!("{ns}.versioned");
    let in_flight = tokio::spawn(async move {
        in_flight_client
            .tools_call(&qualified, json!({}))
            .await
            .expect("the in-flight call must still succeed")
    });

    // Give the in-flight call time to have started (and be mid-`sleep(1)`)
    // before republishing.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let v2 = json!({
        "source": "def main(args):\n    return {\"version\": 2}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "versioned", "kind": "python", "spec": v2}),
        )
        .await
        .expect("republish v2 ok");

    let in_flight_result = in_flight.await.expect("task must not panic");
    assert_eq!(
        extract_structured(&in_flight_result)["version"],
        json!(1),
        "the call already in flight when republish happened must return the OLD source's result"
    );

    let next_result = client
        .tools_call(&format!("{ns}.versioned"), json!({}))
        .await
        .expect("the next call must succeed");
    assert_eq!(
        extract_structured(&next_result)["version"],
        json!(2),
        "a call started after republish must return the NEW source's result"
    );
}
