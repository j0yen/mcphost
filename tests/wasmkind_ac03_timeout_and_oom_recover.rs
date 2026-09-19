//! PRD-mcphost-wasm-kind
//! AC3 -- Given a component that loops forever, and one that allocates past
//! the memory cap, When called, Then each terminates within the budget with
//! the corresponding structured error code, and the process serves the next
//! call normally.

use crate::common;
use common::{TestServer, extract_structured, signup, wasm_fixture_b64, wasm_kind_registry};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn loop_forever_times_out_and_oom_is_capped_and_the_server_still_serves_after() {
    let server = TestServer::start_with_kinds(wasm_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "Wasm AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "spinner",
                "kind": "wasm",
                "spec": {"component": wasm_fixture_b64("loop_forever"), "timeout_s": 1},
            }),
        )
        .await
        .expect("publish loop_forever tool");
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hog",
                "kind": "wasm",
                "spec": {"component": wasm_fixture_b64("oom"), "timeout_s": 5, "memory_mb": 16},
            }),
        )
        .await
        .expect("publish oom tool");
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echoer",
                "kind": "wasm",
                "spec": {"component": wasm_fixture_b64("echo")},
            }),
        )
        .await
        .expect("publish echo tool");

    let started = Instant::now();
    let err = client
        .tools_call(&format!("{ns}.spinner"), json!({}))
        .await
        .expect_err("an infinite loop must not return normally");
    let elapsed = started.elapsed();
    assert_eq!(err.error_code.as_deref(), Some("tool_timeout"));
    assert!(
        elapsed < Duration::from_secs(5),
        "must terminate well within budget, took {elapsed:?}"
    );

    let err = client
        .tools_call(&format!("{ns}.hog"), json!({}))
        .await
        .expect_err("allocating past the memory cap must not return normally");
    assert_eq!(err.error_code.as_deref(), Some("tool_oom"));

    // The process serves the next call normally -- both a fresh call on one
    // of the misbehaving tools' own namespace and an ordinary, well-behaved
    // wasm tool.
    let result = client
        .tools_call(&format!("{ns}.echoer"), json!({"msg": "still alive"}))
        .await
        .expect("the server must keep serving calls after a timeout/oom");
    assert_eq!(
        extract_structured(&result)["payload"],
        json!({"msg": "still alive"})
    );
}
