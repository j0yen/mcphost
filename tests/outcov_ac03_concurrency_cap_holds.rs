//! PRD-mcphost-outsch-ac01-coverage-diagnosable
//! AC3 (P0; v2 AC1) — Given a queue with `concurrency: 2` and 10 pushed items whose
//! consumer sleeps 2 s, When `stats` is polled every 200 ms for 3 s, Then
//! `running` reaches 2 and never exceeds it, the four counts sum to 10 at every
//! poll, and `host.runs.list {trigger: "queue"}` never shows more than 2 runs
//! `queued` or `running`; Given `jobs_concurrent: 1`, Then at most one is
//! `running` at any poll.

use crate::common;
use crate::qprim;
use qprim::{ALL_TASKS, any_object, call, items, publish_consumer, rig, stats};
use serde_json::json;
use std::time::{Duration, Instant};

/// Polls `stats` and the queue-triggered runs every 200 ms for `window`; returns
/// the highest `running` seen, asserting `cap` on it, the sum, and the runs.
async fn poll_cap(client: &common::McpClient, queue: &str, total: u64, cap: u64, window: Duration) -> u64 {
    let end = Instant::now() + window;
    let mut highest = 0;
    while Instant::now() < end {
        let s = stats(client, queue).await;
        let running = s["running"].as_u64().expect("running");
        highest = highest.max(running);
        assert!(running <= cap, "running {running} passed the cap {cap}: {s}");
        let sum: u64 = ["pending", "running", "done", "failed"].iter().map(|k| s[k].as_u64().expect("count")).sum();
        assert_eq!(sum, total, "the four counts must sum to {total}: {s}");
        let runs = call(client, "host.runs.list", json!({"trigger": "queue"})).await;
        let live = runs["runs"]
            .as_array()
            .expect("runs array")
            .iter()
            .filter(|r| matches!(r["status"].as_str(), Some("queued" | "running")))
            .count() as u64;
        assert!(live <= cap, "{live} queue runs queued or running, cap {cap}: {runs}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    highest
}

async fn scenario(jobs_concurrent: i64, cap: u64) -> u64 {
    let rig = rig(ALL_TASKS, |catalog| {
        qprim::free_plan(catalog, |p| p.jobs_concurrent = jobs_concurrent);
    })
    .await;
    publish_consumer(&rig.client, "sleeper", any_object(), 2_000).await;
    call(&rig.client, "host.queue.create", json!({"name": "crawl", "consumer_tool": "sleeper", "concurrency": 2})).await;
    let pushed = call(&rig.client, "host.queue.push", json!({"name": "crawl", "items": items(10)})).await;
    assert_eq!(pushed["accepted"], json!(10), "{pushed}");
    poll_cap(&rig.client, "crawl", 10, cap, Duration::from_secs(3)).await
}

#[tokio::test]
/// v2 AC1
async fn qprim_ac03a_running_reaches_two_and_never_exceeds_it() {
    let highest = scenario(4, 2).await;
    assert_eq!(highest, 2, "running must reach the queue's concurrency of 2");
}

#[tokio::test]
/// v2 AC1
async fn qprim_ac03b_jobs_concurrent_one_holds_at_most_one_running() {
    let highest = scenario(1, 1).await;
    assert_eq!(highest, 1, "one slot is free, so one item runs");
}
