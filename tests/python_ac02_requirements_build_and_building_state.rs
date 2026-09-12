//! AC2 (P0) — Given a tool with `requirements: ["pydantic", "httpx"]`, When
//! published, Then `tools/list` shows it immediately, a call during the
//! build waits (bounded) and reports a structured `building` result if it
//! isn't done in time, and after the build (under 60 s on the reference
//! box) a call succeeds.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn requirements_build_then_call_succeeds() {
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
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import pydantic, httpx\ndef main(args):\n    return {\"pydantic\": pydantic.VERSION, \"has_httpx\": hasattr(httpx, 'get')}\n",
        "requirements": ["pydantic", "httpx"],
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "deps", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // "tools/list shows it immediately" — no build-state gate on listing.
    let listed = client.tools_list().await.expect("tools/list ok");
    let names: Vec<String> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    assert!(
        names.contains(&format!("{ns}.deps")),
        "the tool must be listed immediately after publish, got: {names:?}"
    );

    // The very first call finds no ready/failed environment, kicks off the
    // build in the background, and -- per PRD-mcphost-first-call-reliability
    // requirement 1 -- waits up to MCPHOST_CALL_READY_WAIT_MS (default 20s)
    // for it to become ready before giving up. Either the wait resolves the
    // build in time and the call already succeeds, or it doesn't and the
    // call returns the structured `{status: "building", retry_after_ms,
    // ready_check}` result (requirement 2) instead of the old free-text /
    // `tool_building`-coded error -- both are "a call during the build",
    // just at different points along the wait.
    let first = client
        .tools_call(&format!("{ns}.deps"), json!({}))
        .await
        .expect("a not-yet-ready environment is a structured Ok result now, never an error");
    let first_structured = extract_structured(&first);
    if first_structured["status"] == json!("building") {
        assert!(
            first_structured["retry_after_ms"].as_u64().is_some(),
            "building result must include retry_after_ms, got: {first_structured:?}"
        );
        assert_eq!(
            first_structured["ready_check"]["method"],
            json!("host.tool_call"),
            "building result must name a ready_check RPC, got: {first_structured:?}"
        );
    } else {
        assert!(
            first_structured["has_httpx"].as_bool().unwrap_or(false),
            "if the first call already succeeded (build finished inside the wait \
             bound), it must carry the real result, got: {first_structured:?}"
        );
    }

    let started = Instant::now();
    let result = poll_until_ready(
        &client,
        &format!("{ns}.deps"),
        json!({}),
        Duration::from_secs(90),
    )
    .await
    .unwrap_or_else(|e| panic!("call must eventually succeed: {} {}", e.code, e.message));
    println!(
        "AC2: pydantic+httpx build+call took {:?}",
        started.elapsed()
    );
    let structured = extract_structured(&result);
    assert!(structured["has_httpx"].as_bool().unwrap_or(false));
    assert!(structured["pydantic"].is_string());
}
