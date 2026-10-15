//! PRD-mcphost-docs-telemetry-failed-after-search AC4 (P0) — Given 50
//! concurrent failing calls from 50 tenants each with a prior search, When
//! timed, Then every row is joined once and the p99 added latency on the
//! error path is <= 1 ms.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::state::now_unix;
use serde_json::json;
use std::time::{Duration, Instant};

const TENANTS: usize = 50;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dtf_ac04_fifty_tenants_each_joined_exactly_once() {
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    let mut clients = Vec::new();
    for i in 0..TENANTS {
        let (_ns, key) = signup(&server.base_url, &format!("DTF AC4 {i}")).await;
        clients.push(McpClient::with_bearer(&server.base_url, &key));
    }
    for c in &clients {
        c.tools_call("host.help.search", json!({"q": "publish a tool"})).await.expect("search");
    }
    let mut handles = Vec::new();
    for c in clients {
        handles.push(tokio::spawn(async move {
            c.tools_call("host.tool_publish", json!({"name": "bad", "kind": "echo", "spec": {"schema": 5}}))
                .await
                .expect_err("failing publish");
        }));
    }
    for h in handles {
        h.await.expect("task");
    }

    let mut rows = Vec::new();
    for _ in 0..200 {
        rows = server.state.db.help_search_events_for_test().await.expect("rows");
        if rows.len() == TENANTS && rows.iter().all(|r| r.3.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(rows.len(), TENANTS, "one row per search");
    let mut tenants: Vec<_> = rows.iter().map(|r| r.0).collect();
    tenants.sort();
    tenants.dedup();
    assert_eq!(tenants.len(), TENANTS, "one row per tenant");
    for r in &rows {
        assert_eq!(r.3.as_deref(), Some("invalid_spec"), "{r:?}");
        assert_eq!(r.4.as_deref(), Some("host.tool_publish"), "{r:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dtf_ac04_error_path_hook_p99_is_at_most_one_millisecond() {
    let server = TestServer::start().await;
    let now = now_unix();
    for t in 1..=TENANTS as i64 {
        server.state.db.record_help_search(Some(t), "q".into(), None, now).await.expect("seed");
    }
    crate::perf_budget!(1, {
        let mut samples = Vec::new();
        for t in 1..=TENANTS as i64 {
            let start = Instant::now();
            mcphost::help_search::note_failure(&server.state, t, "invalid_spec", "host.tool_publish");
            samples.push(start.elapsed());
        }
        samples.sort();
        let p99 = samples[(samples.len() * 99).div_ceil(100) - 1];
        assert!(p99 <= Duration::from_millis(1), "p99 added error-path latency {p99:?} exceeds 1 ms");
    });
}
