//! PRD-mcphost-agent-directory
//! AC8 (P1) — Given 1 000 profiled tenants in a warm database, When 200
//! sequential `host.agent.lookup` calls run, Then p95 latency is under
//! 50 ms.
//!
//! Hardware-dependent, like `tests/ac11_load_smoke.rs` (same reference-box
//! caveat) -- `#[ignore]`d by default so an off-reference box never fails
//! the default `cargo test` gate; run directly with `cargo test --release
//! --test suite_core_01 agentdir_ac08:: -- --ignored --nocapture` and read
//! the printed p95 (PRD-mcphost-test-suite-consolidation: this file is
//! `#[path]`-included into a `tests/suite_core_NN.rs` binary).

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

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let idx = ((p * (sorted_ms.len() as f64 - 1.0)).round() as usize).min(sorted_ms.len() - 1);
    sorted_ms[idx]
}

#[tokio::test]
#[ignore = "hardware-dependent load test; run explicitly, see module docs"]
async fn two_hundred_sequential_lookups_stay_under_50ms_p95() {
    let data_dir = std::env::temp_dir().join(format!("mcphost-agentdir-ac08-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).unwrap();

    // Seed 1000 profiled tenants directly, same "seed directly" rationale
    // as ac11_load_smoke.rs: this AC is about lookup latency, not signup
    // throughput.
    const N: usize = 1000;
    let db = Db::open(&data_dir).expect("open db for seeding");
    db.migrate().await.expect("migrate");
    let mut namespaces = Vec::with_capacity(N);
    for i in 0..N {
        let key = generate_key();
        let namespace = generate_namespace();
        let tenant = db
            .create_tenant(format!("Latency Tenant {i}"), namespace.clone(), hash_key(&key), None)
            .await
            .expect("seed tenant");
        db.set_agent_profile(
            tenant.id,
            Some(Some(format!("lat{i:04}"))),
            Some(Some("seeded for AC8".to_string())),
            Some(vec!["latency".to_string()]),
            None,
        )
        .await
        .map(|_| ())
        .expect("seed profile");
        namespaces.push(namespace);
    }
    drop(db);

    let port = free_port();
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let mut child = Command::new(bin)
        .arg("serve")
        .env("MCPHOST_DATA_DIR", &data_dir)
        .env("MCPHOST_BIND", format!("127.0.0.1:{port}"))
        .env("MCPHOST_ADMIN_KEY", "agentdir-ac08-admin-key")
        .env("MCPHOST_SECRET_KEY", "agentdir-ac08-secret-key")
        .env("MCPHOST_LOG_LEVEL", "warn")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcphost serve");
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

    // One caller tenant, authenticated, doing 200 sequential lookups of the
    // first 200 seeded namespaces.
    let looker_key = {
        let db = Db::open(&data_dir).expect("reopen db for looker");
        let key = generate_key();
        db.create_tenant("Looker".to_string(), generate_namespace(), hash_key(&key), None)
            .await
            .expect("create looker tenant");
        key
    };

    let mut durations = Vec::with_capacity(200);
    for namespace in namespaces.iter().take(200) {
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {
                "name": "host.agent.lookup",
                "arguments": {"address": namespace},
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
            .header("Mcp-Name", "host.agent.lookup")
            .header("Authorization", format!("Bearer {looker_key}"))
            .json(&body)
            .send()
            .await
            .expect("lookup request");
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(resp.status().is_success(), "lookup for {namespace} failed: {}", resp.status());
        durations.push(elapsed_ms);
    }

    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = percentile(&durations, 0.95);
    println!("AC8 lookup latency: 200 sequential host.agent.lookup calls, p95={p95:.2}ms");
    assert!(p95 < 50.0, "p95 {p95:.2}ms exceeds the 50ms target");

    let _ = std::fs::remove_dir_all(&data_dir);
}
