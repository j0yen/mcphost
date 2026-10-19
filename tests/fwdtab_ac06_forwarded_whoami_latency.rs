//! PRD-mcphost-generated-dispatch-table-stack-budget
//! AC6 (P1) — Given 200 sequential `host.tool_call name="host.whoami"` calls
//! against the in-process server, When `fwdtab_ac06` prints p50 and p95 on
//! `origin/main` (with #413) and on the branch on the same host with the same
//! `-j`, Then the branch p95 is <= 1.05 x the main p95 and both printed values
//! appear in the PR body.
//!
//! The comparison is between two builds, so this test asserts no wall-clock
//! bound (a single run cannot know the other build's number): it checks every
//! call forwarded and prints the two percentiles the PR body records:
//! `cargo test --test suite_core_NN fwdtab_ac06 -- --nocapture`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::{Duration, Instant};

const CALLS: usize = 200;

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let idx = ((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

#[tokio::test]
async fn fwdtab_ac06_two_hundred_forwarded_whoami_calls_print_p50_and_p95() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC6 Latency Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let args = json!({"name": "host.whoami", "args": {}});

    // One warm-up call, then the 200 timed ones.
    client.tools_call("host.tool_call", args.clone()).await.expect("warm-up forward");
    let mut samples = Vec::with_capacity(CALLS);
    for _ in 0..CALLS {
        let started = Instant::now();
        let call = client.tools_call("host.tool_call", args.clone()).await.expect("forwarded host.whoami");
        samples.push(started.elapsed());
        assert_eq!(extract_structured(&call)["forwarded_to"], "host.whoami", "{call}");
    }
    samples.sort();
    println!(
        "fwdtab_ac06: {CALLS} sequential host.tool_call name=host.whoami: p50={:.3} ms p95={:.3} ms",
        percentile(&samples, 0.50).as_secs_f64() * 1000.0,
        percentile(&samples, 0.95).as_secs_f64() * 1000.0
    );
}
