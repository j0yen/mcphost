//! AC8 (PRD-mcphost-signup-kill-switch-and-source) — Given 50 concurrent
//! signups while the pause file is toggled on mid-burst, When the burst
//! completes, Then every response is either success or `signup_paused`,
//! never a 500, and tenant count equals the success count.
//!
//! The burst is split into two concurrent waves straddling the toggle
//! rather than one undifferentiated 50-way spawn: a request racing the
//! `std::fs::write` below could land on either side of it, so an
//! unsplit burst can't tell "the pause check ran and let everything
//! through" apart from "the pause check doesn't exist" -- both produce
//! `ok=50, paused=0`. Wave 1 is awaited to completion *before* the
//! toggle (so it's a deterministic control: pause must not affect it),
//! and wave 2 is only spawned *after* the toggle (so every one of its
//! responses is deterministically required to be `signup_paused` --
//! deleting the pause gate in src/control.rs turns those into 500-free
//! successes and this test fails).

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn concurrent_burst_with_pause_toggle_never_500s() {
    // A high enough per-IP cap that the rate limiter (unrelated to this
    // AC) never masks the pause behavior under test -- every request in
    // this burst comes from the same loopback address.
    let server = TestServer::start_with_signup_rate_limit(1000).await;

    const WAVE1: usize = 10;
    const WAVE2: usize = 40;

    // Wave 1: fired concurrently and awaited to completion before the
    // pause file exists at all -- every one of these must succeed.
    let mut wave1 = Vec::with_capacity(WAVE1);
    for i in 0..WAVE1 {
        let base_url = server.base_url.clone();
        wave1.push(tokio::spawn(async move {
            let client = McpClient::new(&base_url);
            client
                .tools_call("signup", json!({"name": format!("Burst Agent pre-{i}")}))
                .await
        }));
    }
    let mut ok = 0;
    for handle in wave1 {
        match handle.await.expect("task must not panic") {
            Ok(_) => ok += 1,
            Err(e) => panic!("unexpected error before pause toggle: {} {}", e.code, e.message),
        }
    }
    assert_eq!(ok, WAVE1, "every pre-toggle response must succeed");

    // Toggle the pause file on mid-burst.
    std::fs::write(server.state.signup_pause.path(), "burst pause\n").expect("write pause file");

    // Wave 2: fired concurrently only after the toggle -- `signup_pause`
    // is read fresh off disk on every call (never cached), so every one
    // of these must come back `signup_paused`, never a success and
    // never a 500.
    let mut wave2 = Vec::with_capacity(WAVE2);
    for i in 0..WAVE2 {
        let base_url = server.base_url.clone();
        wave2.push(tokio::spawn(async move {
            let client = McpClient::new(&base_url);
            client
                .tools_call("signup", json!({"name": format!("Burst Agent post-{i}")}))
                .await
        }));
    }
    let mut paused = 0;
    for handle in wave2 {
        match handle.await.expect("task must not panic") {
            Ok(_) => panic!("signup succeeded despite the pause file being present"),
            Err(e) if e.error_code.as_deref() == Some("signup_paused") => paused += 1,
            Err(e) => panic!("unexpected error, never a 500-shaped failure: {} {}", e.code, e.message),
        }
    }
    assert_eq!(paused, WAVE2, "every post-toggle response must be signup_paused");

    assert_eq!(ok + paused, WAVE1 + WAVE2, "every response must be success or signup_paused");

    let tenants = server.state.db.list_tenants().await.expect("list tenants");
    assert_eq!(
        tenants.len(),
        ok,
        "tenant count must equal the success count"
    );
}
