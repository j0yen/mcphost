//! AC4 (P0) — Given a dependency-free python tool of corpus-task size in
//! the local sandbox, When published and called with instrumentation on,
//! Then measured publish ≤ 10 s and first call ≤ 5 s, and the receipt
//! records the pre-fix measurements beside them.
//!
//! ## Receipt
//!
//! See `docs/receipts/python-kind-latency.md` for the full pre/post note.
//! Summary: the PRD's recorded baseline (panel_rag_indexer_03, a different
//! synthorg session than AC1's fixture) measured 88.4s publish / 95.3s
//! first-call on the production box class (`mcphost-1`, a Hetzner ccx13).
//! That box's `uv` almost certainly had to fetch/build a managed CPython
//! interpreter on a cold cache -- `uv venv` on THIS dev box (RedBaron,
//! `uv` and `/usr/bin/python3` both already warm) takes single-digit
//! milliseconds, so the dominant term (per the PRD's own "measure before
//! cutting" instruction) is a one-time, box-local interpreter-provisioning
//! cost that this fixture cannot reproduce without an actually-cold `uv`
//! cache. This test's own measured numbers on a warm cache are the
//! honestly-reproducible half of the picture; the instrumentation this PRD
//! adds (`publish_ms`/`cold_call_ms` on the `tracing::info!` lines in
//! `validate_async`/`call`) is what would catch a regression toward the
//! baseline's numbers on any box, warm cache or not -- per the PRD's own
//! Open Questions: "if the dominant term is hardware-bound the receipt
//! says so and the target is revisited rather than gamed."

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn publish_and_first_call_meet_budget_on_a_warm_sandbox() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Corpus-task size and shape: a small, dependency-free retrieval-style
    // tool, comparable to the recorded `retrieval_latency_check` fixture
    // (no third-party requirements -- the fast path this AC targets).
    let source = "\
import time

def main(args):
    query = args.get('query', 'default query')
    top_k = args.get('top_k', 5)
    start = time.perf_counter()
    time.sleep(0.01)
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    return {'latency_ms': round(elapsed_ms, 2), 'query': query, 'top_k': top_k}
";
    let spec = json!({"source": source, "args_schema": {
        "type": "object",
        "properties": {
            "query": {"type": "string"},
            "top_k": {"type": "integer", "default": 5},
        },
    }});

    let publish_started = Instant::now();
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "retrieval_latency_check", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let publish_elapsed = publish_started.elapsed();
    println!("[receipt] measured publish latency: {publish_elapsed:?}");
    assert!(
        publish_elapsed <= Duration::from_secs(10),
        "publish must complete within 10s, took {publish_elapsed:?}"
    );

    let call_started = Instant::now();
    let result = poll_until_ready(
        &client,
        &format!("{ns}.retrieval_latency_check"),
        json!({"query": "SOC 2 Type II audit evidence retention policy 2024", "top_k": 5}),
        Duration::from_secs(30),
    )
    .await
    .expect("first call must eventually succeed once the env is ready");
    let call_elapsed = call_started.elapsed();
    println!("[receipt] measured first-call latency (incl. tool_building polling): {call_elapsed:?}");
    assert!(result["structuredContent"]["latency_ms"].is_number());
    assert!(
        call_elapsed <= Duration::from_secs(5),
        "first successful call must complete within 5s of the env being ready \
         (measured including the tool_building poll loop), took {call_elapsed:?}"
    );
}
