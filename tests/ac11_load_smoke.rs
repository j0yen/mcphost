//! AC11 (non-functional) — Given 200 concurrent `tools/call` of `echo`
//! from 200 tenants on the reference box, When measured, Then p95 is
//! under 50 ms, every call succeeds, and resident memory stays under
//! 100 MiB.
//!
//! This spawns the real `mcphost` binary as a subprocess (so RSS reflects
//! the server process alone, not the test harness) against a reference
//! box the PRD names as a Hetzner CPX21; this box's own hardware is
//! whatever `/build` runs on, not that reference. Per the PRD instructions
//! ("may be smoke commands recorded in verification instead of unit
//! tests"), this is `#[ignore]`d by default so an off-reference box's
//! numbers never fail the `cargo test --release` gate — run it directly
//! with `cargo test --release --test suite_core_01 ac11_load_smoke:: --
//! --ignored --nocapture` (PRD-mcphost-test-suite-consolidation: this file
//! is `#[path]`-included into `tests/suite_core_01.rs`, so `--test` now
//! names that suite binary and the module-qualified filter narrows it to
//! this file) and read the printed p95/RSS.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use serde_json::json;

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().unwrap().port()
}

fn rss_bytes(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let idx = ((p * (sorted_ms.len() as f64 - 1.0)).round() as usize).min(sorted_ms.len() - 1);
    sorted_ms[idx]
}

#[tokio::test]
#[ignore = "hardware-dependent load test; run explicitly, see module docs"]
async fn two_hundred_concurrent_echo_calls() {
    let data_dir = std::env::temp_dir().join(format!("mcphost-ac11-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).unwrap();

    // Seed 200 tenants directly (signup is rate-limited to 5/hour/IP, and
    // this AC is about call concurrency, not signup throughput).
    const N: usize = 200;
    let db = Db::open(&data_dir).expect("open db for seeding");
    db.migrate().await.expect("migrate");
    let mut tenants = Vec::with_capacity(N);
    for i in 0..N {
        let key = generate_key();
        let namespace = generate_namespace();
        db.create_tenant(
            format!("Load Tenant {i}"),
            namespace.clone(),
            hash_key(&key),
            None,
        )
        .await
        .expect("seed tenant");
        tenants.push((namespace, key));
    }
    drop(db);

    let port = free_port();
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let mut child = Command::new(bin)
        .arg("serve")
        .env("MCPHOST_DATA_DIR", &data_dir)
        .env("MCPHOST_BIND", format!("127.0.0.1:{port}"))
        .env("MCPHOST_ADMIN_KEY", "ac11-admin-key")
        .env("MCPHOST_SECRET_KEY", "ac11-secret-key")
        .env("MCPHOST_LOG_LEVEL", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcphost serve");
    let pid = child.id();
    let stderr = child.stderr.take();
    let _child = ChildGuard(child);

    let base_url = format!("http://127.0.0.1:{port}");
    let http = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if reqwest::get(format!("{base_url}/healthz")).await.is_ok() {
            break;
        }
        if Instant::now() > deadline {
            let mut buf = String::new();
            if let Some(mut s) = stderr {
                let _ = s.read_to_string(&mut buf);
            }
            panic!("mcphost serve did not become healthy in time; stderr:\n{buf}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Publish `echo` under every tenant before the timed window.
    for (ns, key) in &tenants {
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {
                "name": "host.tool_publish",
                "arguments": {"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        });
        let resp = http
            .post(format!("{base_url}/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call")
            .header("Mcp-Name", "host.tool_publish")
            .header("Authorization", format!("Bearer {key}"))
            .json(&body)
            .send()
            .await
            .expect("publish request");
        assert!(
            resp.status().is_success(),
            "publish for {ns} failed: {}",
            resp.status()
        );
    }

    // The timed window: 200 concurrent echo calls, one per tenant.
    let mut handles = Vec::with_capacity(N);
    for (ns, key) in tenants.clone() {
        let http = http.clone();
        let base_url = base_url.clone();
        handles.push(tokio::spawn(async move {
            let qualified = format!("{ns}.hello");
            let body = json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {
                    "name": qualified,
                    "arguments": {},
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                        "io.modelcontextprotocol/clientCapabilities": {},
                    },
                },
            });
            let start = Instant::now();
            let resp = http
                .post(format!("{base_url}/mcp"))
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2026-07-28")
                .header("Mcp-Method", "tools/call")
                .header("Mcp-Name", qualified)
                .header("Authorization", format!("Bearer {key}"))
                .json(&body)
                .send()
                .await;
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            let ok = resp.map(|r| r.status().is_success()).unwrap_or(false);
            (ok, elapsed_ms)
        }));
    }

    let mut durations = Vec::with_capacity(N);
    let mut errors = 0usize;
    for h in handles {
        let (ok, ms) = h.await.expect("join");
        if !ok {
            errors += 1;
        }
        durations.push(ms);
    }
    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = percentile(&durations, 0.95);
    let rss = rss_bytes(pid).unwrap_or(0);

    println!(
        "AC11 smoke: {N} concurrent echo calls, errors={errors}, p95={p95:.2}ms, rss={:.1}MiB",
        rss as f64 / (1024.0 * 1024.0)
    );

    assert_eq!(errors, 0, "every call must succeed");
    // Recorded, not hard-asserted at reference-box thresholds: see module
    // docs. Still asserts a generous bound so a genuine regression (not
    // just off-reference-hardware slack) fails the (explicitly-run) test.
    assert!(
        p95 < 500.0,
        "p95 {p95:.2}ms is far outside even a generous bound"
    );
    assert!(
        rss < 300 * 1024 * 1024,
        "RSS {rss} bytes is far outside even a generous bound"
    );

    let _ = std::fs::remove_dir_all(&data_dir);
}
