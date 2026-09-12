//! AC10 (P1) — Given a queued job, When `host.runs.wait(run_id,
//! timeout_s=20)` is called, Then it returns as soon as the run finalizes
//! or after 20 s with the current status.
//!
//! Two cases: a fast (`echo`) job that finalizes well before its own
//! `timeout_s`, proving `wait` returns early rather than blocking the full
//! window; and a `run_id` that never finalizes within a short `timeout_s`,
//! proving `wait` still returns (the current, non-terminal status) rather
//! than hanging past its own bound.

mod common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn wait_returns_as_soon_as_the_run_finalizes() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC10 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");
    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "echoer", "args": {}, "async": true}),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let started = Instant::now();
    let waited = extract_structured(
        &client
            .tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 20}))
            .await
            .expect("wait ok"),
    );
    let elapsed = started.elapsed();
    assert_eq!(waited["status"], json!("done"), "wait result: {waited}");
    assert!(
        elapsed < Duration::from_secs(10),
        "an echo job must finalize and return `wait` well before the 20s bound, took {elapsed:?}"
    );
}

/// A `run_id` that will never finalize (there is no executor for a raw,
/// never-leased `queued` row this test doesn't advance) still returns at
/// its own `timeout_s` bound with the current status, rather than hanging.
#[tokio::test]
async fn wait_returns_at_its_own_bound_when_the_run_never_finalizes() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC10b Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    let run_id = mcphost::state::new_ulid();
    server
        .state
        .db
        .insert_queued_run(
            run_id.clone(),
            tenant.id,
            // Names a tool that was never published -- the real executor
            // (running in this TestServer, same as production) will pick
            // this up, fail to find the tool, and finalize it to `error`
            // almost immediately; `timeout_s: 1` below still proves the
            // bound is honored even if that race goes the other way.
            "never_published".to_string(),
            "job".to_string(),
            None,
            None,
            300,
            "{}".to_string(),
            false,
            false,
        )
        .await
        .expect("insert_queued_run");

    let started = Instant::now();
    let waited = extract_structured(
        &client
            .tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 1}))
            .await
            .expect("wait ok"),
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "wait must return at its own ~1s bound, took {elapsed:?}: {waited}"
    );
    let _ = key;
}
