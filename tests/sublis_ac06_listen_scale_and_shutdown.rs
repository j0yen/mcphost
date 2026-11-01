//! solo-suite: 60 one process-wide drain and a /proc/self/fd baseline: no other test may share this binary
//!
//! AC6 — Given 200 concurrent listen streams across 20 tenants on one process,
//! When each tenant publishes 5 tools and the server then shuts down, Then
//! every stream receives every notification for its own tenant with p95
//! delivery at most 1 s, every stream still open at shutdown receives the
//! `complete` result, the hub then holds 0 entries, and the `/proc/self/fd`
//! count is back to its pre-test baseline after a 2 s settle.
//!
//! Run as its own binary with `--test-threads=1`: the drain is process-global
//! and the fd count is process-wide.

use crate::common;
use common::{ListenStream, McpClient, bare_app_state, signup};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const TENANTS: usize = 20;
const STREAMS_PER_TENANT: usize = 10;
const PUBLISHES: usize = 5;

fn fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd").expect("/proc/self/fd").count()
}

/// Every message of one stream, stamped on arrival, until the server ends it.
async fn read_until_closed(mut stream: ListenStream, notified: Arc<AtomicUsize>) -> Vec<(Instant, Value)> {
    let mut seen = Vec::new();
    while let Some(msg) = stream.next_message(Duration::from_secs(60)).await {
        if msg["method"] == json!("notifications/tools/list_changed") {
            notified.fetch_add(1, Ordering::SeqCst);
        }
        seen.push((Instant::now(), msg));
    }
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sublis_ac06_two_hundred_streams_get_every_notification_and_complete_at_shutdown() {
    let (mut state, _dir) = bare_app_state().await;
    state.signup_rate_limit_per_hour = 10_000;
    let state = Arc::new(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(mcphost::http::serve_on_listener_until(listener, state.clone(), async move {
        let _ = stopped.await;
    }));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut keys = Vec::new();
    for i in 0..TENANTS {
        keys.push(signup(&base, &format!("sublis6-{i}")).await.1);
    }
    // One warm call per tenant so lazily opened db connections are in the baseline.
    for key in &keys {
        McpClient::with_bearer(&base, key).tools_list().await.expect("warm tools/list");
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let baseline = fd_count();

    // 10 streams per tenant, opened concurrently and each acknowledged.
    let notified = Arc::new(AtomicUsize::new(0));
    let mut opens = Vec::new();
    for (t, key) in keys.iter().enumerate() {
        for s in 0..STREAMS_PER_TENANT {
            let (base, key) = (base.clone(), key.clone());
            opens.push(tokio::spawn(async move {
                let mut stream = ListenStream::open(&base, &key, (t * 100 + s + 1) as u64).await;
                assert_eq!(stream.status, 200);
                let first = stream.next_message(Duration::from_secs(30)).await.expect("acknowledgement");
                assert_eq!(first["method"], json!("notifications/subscriptions/acknowledged"), "{first}");
                (t, stream, first)
            }));
        }
    }
    let mut readers = Vec::new();
    for open in opens {
        let (t, stream, first) = open.await.expect("open task");
        let notified = notified.clone();
        readers.push((
            t,
            tokio::spawn(async move {
                let mut rest = read_until_closed(stream, notified).await;
                rest.insert(0, (Instant::now(), first));
                rest
            }),
        ));
    }
    // The acknowledgement goes out before `listen` registers with the hub.
    for _ in 0..100 {
        if state.listen_hub.entries() == TENANTS {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(state.listen_hub.entries(), TENANTS);
    let registered_by = Instant::now() + Duration::from_secs(30);
    loop {
        let open: usize = (1..=TENANTS as i64).map(|id| state.listen_hub.open_for(id)).sum();
        if open == TENANTS * STREAMS_PER_TENANT || Instant::now() > registered_by {
            assert_eq!(open, TENANTS * STREAMS_PER_TENANT, "every stream is registered");
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Each tenant publishes 5 tools; the instant each call returned is kept.
    let mut publishers = Vec::new();
    for key in keys.clone() {
        let base = base.clone();
        publishers.push(tokio::spawn(async move {
            let client = McpClient::with_bearer(&base, &key);
            let mut returned = Vec::new();
            for n in 0..PUBLISHES {
                client
                    .tools_call(
                        "host.tool.publish",
                        json!({"name": format!("t{n}"), "kind": "echo", "spec": {"schema": {"type": "object"}}}),
                    )
                    .await
                    .expect("publish");
                returned.push(Instant::now());
            }
            returned
        }));
    }
    let mut returned = Vec::new();
    for p in publishers {
        returned.push(p.await.expect("publisher"));
    }
    let want = TENANTS * STREAMS_PER_TENANT * PUBLISHES;
    let waited = Instant::now();
    while notified.load(Ordering::SeqCst) < want && waited.elapsed() < Duration::from_secs(30) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(notified.load(Ordering::SeqCst), want, "every stream got every notification");

    // Shut the server down: every stream still open ends with the completion result.
    let _ = stop.send(());
    let drained = tokio::time::timeout(Duration::from_secs(30), server).await.expect("the drain ends").expect("server task");
    assert!(drained.expect("serve ok"), "a drain ran");

    let mut delays = Vec::new();
    for (t, reader) in readers {
        let seen = reader.await.expect("reader");
        let methods: Vec<&str> = seen.iter().filter_map(|(_, m)| m["method"].as_str()).collect();
        assert_eq!(methods.first(), Some(&"notifications/subscriptions/acknowledged"), "tenant {t}: {methods:?}");
        let changes: Vec<&Instant> = seen
            .iter()
            .filter(|(_, m)| m["method"] == json!("notifications/tools/list_changed"))
            .map(|(at, _)| at)
            .collect();
        assert_eq!(changes.len(), PUBLISHES, "tenant {t} stream got {PUBLISHES} notifications: {methods:?}");
        for (k, at) in changes.iter().enumerate() {
            delays.push(at.saturating_duration_since(returned[t][k]));
        }
        let (_, last) = seen.last().expect("a final message");
        assert_eq!(last["result"]["resultType"], json!("complete"), "tenant {t}: the stream ends with the completion result: {last}");
    }
    delays.sort();
    let p95 = delays[(delays.len() * 95).div_ceil(100) - 1];
    let p95_ms = p95.as_millis();
    let p95_within_budget = p95_ms <= 1_000;
    assert!(p95_within_budget, "p95 delivery {p95_ms} ms");

    assert_eq!(state.listen_hub.entries(), 0, "the hub holds nothing after shutdown");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after = fd_count();
    assert!(after <= baseline, "fd count {after} is back to the baseline {baseline}");
}
