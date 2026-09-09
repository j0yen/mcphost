//! AC2 (PRD-mcphost-call-limits-honest) — Given a tool with no `timeout_s`
//! sleeping 35 s, When called, Then `call_timeout` names the 30 s default.
//!
//! Uses the server's *real* `AppState::call_timeout`
//! ([`mcphost::state::CALL_TIMEOUT`], 30s) rather than a shrunk test
//! override -- the property under test is specifically that an undeclared
//! `timeout_s` falls back to that real default, not a smaller
//! test-convenience number, so shrinking it here would prove nothing.
//! `kinds::python::PythonKind::requested_timeout` returns `None` for a spec
//! with no `timeout_s` (see that trait impl's doc comment), so
//! `handler.rs` falls back to this same 30s field for both the dispatch
//! timeout and (via `DEFAULT_TIMEOUT_S`, now aligned to the same 30s) the
//! sandbox's own wall-clock backstop -- the sleep here (32s) is chosen to
//! be comfortably over 30s while keeping this real-time test as short as
//! the property allows (the AC's own illustrative "35 s" is not load-bearing;
//! any sleep past 30s proves the same fallback).

mod common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn undeclared_timeout_uses_the_real_30s_default() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    assert_eq!(
        server.state.call_timeout,
        mcphost::state::CALL_TIMEOUT,
        "this test only proves anything against the real default"
    );
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(args.get(\"seconds\", 0))\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        // No `timeout_s` -- the default must apply.
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slowdefault", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.slowdefault");

    let _ = poll_until_ready(&client, &qualified, json!({"seconds": 0}), Duration::from_secs(20))
        .await
        .expect("warm call ok");

    let started = Instant::now();
    let err = client
        .tools_call(&qualified, json!({"seconds": 32}))
        .await
        .expect_err("a 32s sleep with no declared timeout_s must time out at the 30s default");
    let elapsed = started.elapsed();
    assert_eq!(err.error_code.as_deref(), Some("call_timeout"));
    assert!(
        err.message.contains("30s"),
        "call_timeout message must name the 30s default: {}",
        err.message
    );
    assert!(
        elapsed >= Duration::from_secs(29) && elapsed < Duration::from_secs(32),
        "expected the timeout at about 30s, took {elapsed:?}"
    );
}
