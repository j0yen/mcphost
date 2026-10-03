//! PRD-mcphost-public-tool-url
//! AC6 (P0) — Given a tool whose run takes 30 s, When the URL is called,
//! Then 202 with `run_id` and `poll` is returned within 26 s, and
//! `GET <poll>` returns the result once the run completes.
//!
//! A real 30s sleep would make this test glacial for a property that
//! doesn't depend on the sleep's actual length (same rationale
//! `tests/runs_ac01_async_job_runs_progress_and_result.rs`'s own doc
//! comment gives for its shortened sleep): this test shrinks
//! `AppState::public_url_sync_deadline` instead of waiting out the real
//! 25s default, with a `Kind::call` that sleeps a couple of real seconds
//! -- same "test-only `Kind` implemented directly in the test file"
//! precedent as `tests/ac15_call_timeout.rs`'s `HangKind`.
//!
//! `TestServer`'s own builder chain has no seam for overriding
//! `public_url_sync_deadline` without threading a new parameter through
//! a dozen existing `start_*` helpers, so this test builds its server
//! directly off `common::bare_app_state()` (an owned `AppState`, mutable
//! before it's wrapped in `Arc`) instead, spawning the executor and HTTP
//! listener itself the same way `TestServer::start_full_with_email`
//! already does internally.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, signup};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct SlowKind;

#[async_trait]
impl Kind for SlowKind {
    fn name(&self) -> &'static str {
        "slow"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "slow".to_string(),
            description: "sleeps a couple of seconds, then returns ok".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn call(&self, _spec: &Value, _args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(json!({"ok": true}))
    }
}

#[tokio::test]
async fn long_run_returns_202_then_poll_resolves_once_done() {
    let (mut state, _data_dir) = common::bare_app_state().await;
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(SlowKind));
    state.kinds = kinds;
    // Shrink the threshold well below the kind's own 2s sleep, so the
    // first request reliably lands on the 202 branch without needing a
    // real 25s wait.
    state.public_url_sync_deadline = Duration::from_millis(200);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let base_url = format!("http://{addr}");
    state.public_url = base_url.clone();
    let state = Arc::new(state);

    mcphost::runs::spawn_executor((*state).clone());
    let serve_state = state.clone();
    tokio::spawn(async move {
        let _ = mcphost::http::serve_on_listener(listener, serve_state).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (_ns, key) = signup(&base_url, "AC6 Tenant").await;
    let client = McpClient::with_bearer(&base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow_job", "kind": "slow", "spec": {}}),
        )
        .await
        .expect("publish ok");
    let shared = common::extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "slow_job", "visibility": "url"}))
            .await
            .expect("tool_share ok"),
    );
    let url = shared["url"].as_str().expect("url field").to_string();

    let http = reqwest::Client::new();
    let started = Instant::now();
    let resp = http.post(&url).body("{}").send().await.expect("POST");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(26),
        "202 must be returned within 26s, took {elapsed:?}"
    );
    assert_eq!(resp.status(), 202, "a run that outlasts the sync deadline must answer 202");
    let body: Value = resp.json().await.expect("json body");
    assert_eq!(body["ok"], json!(true));
    let run_id = body["run_id"].as_str().expect("run_id").to_string();
    let poll = body["poll"].as_str().expect("poll url").to_string();
    assert!(poll.ends_with(&format!("/runs/{run_id}")), "poll url must address this run_id: {poll}");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let poll_resp = http.get(&poll).send().await.expect("GET poll");
        if poll_resp.status() == 200 {
            let poll_body: Value = poll_resp.json().await.expect("json body");
            assert_eq!(poll_body["ok"], json!(true));
            assert_eq!(poll_body["result"], json!({"ok": true}));
            assert_eq!(poll_body["run_id"], json!(run_id));
            break;
        }
        assert_eq!(poll_resp.status(), 202, "while still running, poll must answer 202");
        if Instant::now() >= deadline {
            panic!("run {run_id} never finished within 10s");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
