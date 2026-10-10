//! PRD-mcphost-shared-tool-spec-readback
//! AC8 (P0) — Given a 64 KiB spec, When 50 sharees read it concurrently,
//! Then p95 < 100 ms and every response is identical.
//!
//! Hardware-dependent, like `tests/ac11_load_smoke.rs` (same reference-box
//! caveat) -- `#[ignore]`d by default so running this file alongside every
//! other test binary's own concurrency never fails the default `cargo
//! test` gate on CPU contention that has nothing to do with this PRD's own
//! code; run directly with `cargo test --test suite_sandbox_02
//! mcphost_shared_tool_spec_readback_ac08:: -- --ignored --nocapture` and
//! read the printed p95 (PRD-mcphost-test-suite-consolidation: this file
//! is `#[path]`-included into a `tests/suite_sandbox_NN.rs` binary).

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const SHAREES: usize = 50;

/// A python spec whose `source` is padded with a comment so the whole
/// spec sits just under `mcphost::state::MAX_SPEC_BYTES` (64 KiB) --
/// `host.tool_publish` rejects anything over that ceiling, so this must
/// stay under it while still exercising a genuinely large spec.
fn spec_near_64kib() -> Value {
    let pad = "x".repeat(64 * 1024 - 512);
    let source = format!("# {pad}\ndef main(args):\n    return {{\"ok\": True}}\n");
    let spec = json!({"source": source, "args_schema": {"type": "object"}});
    let size = serde_json::to_vec(&spec).expect("spec serializes").len();
    assert!(
        size <= mcphost::state::MAX_SPEC_BYTES && size > mcphost::state::MAX_SPEC_BYTES - 2048,
        "expected a spec within 2 KiB of the {}-byte ceiling, got {size} bytes",
        mcphost::state::MAX_SPEC_BYTES
    );
    spec
}

async fn read_spec(base_url: String, key: String, tool: String) -> (Duration, Value) {
    let client = McpClient::with_bearer(&base_url, &key);
    let started = Instant::now();
    let result = client
        .tools_call("host.tool_spec_shared", json!({"tool": tool}))
        .await
        .unwrap_or_else(|e| panic!("tool_spec_shared failed: {} {}", e.code, e.message));
    (started.elapsed(), extract_structured(&result))
}

// Every other test in this crate uses the default (single-threaded)
// `#[tokio::test]` runtime -- fine for sequential I/O, but it would
// multiplex all 50 concurrent HTTP round trips (plus the TestServer's own
// request handling) onto one OS thread, defeating the point of a
// genuinely concurrent p95 measurement. This one test opts into a real
// multi-threaded runtime so 50 tasks actually run in parallel.
#[tokio::test(flavor = "multi_thread", worker_threads = 16)]
#[ignore = "hardware-dependent load test; run explicitly, see module docs"]
async fn fifty_concurrent_reads_of_a_64kib_spec_stay_under_100ms_p95_and_agree() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    // 1 owner + 50 sharees, well over the default 5-per-hour signup limit.
    let server = TestServer::start_full_with_signup_rate_limit(
        Some(ADMIN_KEY.to_string()),
        python_kind_registry(&envs_dir.0),
        mcphost::state::CALL_TIMEOUT,
        None,
        (SHAREES as i64) + 10,
    )
    .await;

    let (ns_o, key_o) = signup(&server.base_url, "AC8 Owner").await;
    let client_o = McpClient::with_bearer(&server.base_url, &key_o);
    client_o
        .tools_call("host.group.create", json!({"name": "builders"}))
        .await
        .expect("group.create");

    client_o
        .tools_call(
            "host.tool_publish",
            json!({"name": "big_spec", "kind": "python", "spec": spec_near_64kib()}),
        )
        .await
        .expect("publish ok");
    client_o
        .tools_call(
            "host.tool_share",
            json!({
                "name": "big_spec",
                "visibility": "group",
                "group": "builders",
                "expose_spec": true,
            }),
        )
        .await
        .expect("share ok");

    let mut sharee_keys = Vec::with_capacity(SHAREES);
    for i in 0..SHAREES {
        let (ns, key) = signup(&server.base_url, &format!("AC8 Sharee {i}")).await;
        client_o
            .tools_call("host.group.add", json!({"name": "builders", "namespace": ns}))
            .await
            .expect("group.add");
        sharee_keys.push(key);
    }

    let qualified = format!("{ns_o}.big_spec");
    let handles: Vec<_> = sharee_keys
        .iter()
        .map(|key| {
            tokio::spawn(read_spec(
                server.base_url.clone(),
                key.clone(),
                qualified.clone(),
            ))
        })
        .collect();

    let mut durations = Vec::with_capacity(SHAREES);
    let mut responses = Vec::with_capacity(SHAREES);
    for h in handles {
        let (duration, response) = h.await.expect("read task panicked");
        durations.push(duration);
        responses.push(response);
    }

    let first = &responses[0];
    for (i, response) in responses.iter().enumerate() {
        assert_eq!(response, first, "sharee {i}'s response must be identical to sharee 0's");
    }

    durations.sort();
    let p95 = durations[(SHAREES * 95 / 100).min(SHAREES - 1)];
    assert!(
        common::perf_skipped() || p95 < Duration::from_millis(100),
        "p95 over {SHAREES} concurrent reads of a 64 KiB spec was {p95:?}, expected < 100ms"
    );
}
