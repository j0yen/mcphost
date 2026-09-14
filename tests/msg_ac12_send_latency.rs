//! PRD-mcphost-agent-inbox
//! AC12 (P1) — Given a warm database with 10 000 messages, When 200
//! sequential `host.msg.send` calls run, Then p95 latency is under
//! 100 ms.
//!
//! Hardware-dependent, same reference-box caveat as
//! `agentdir_ac08_lookup_latency.rs` (this file's direct template) --
//! `#[ignore]`d by default; run with `cargo test --release --test
//! suite_core_NN msg_ac12:: -- --ignored --nocapture` and read the
//! printed p95.

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
async fn two_hundred_sequential_sends_stay_under_100ms_p95_warm() {
    let data_dir = std::env::temp_dir().join(format!("mcphost-msg-ac12-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).unwrap();

    // Seed 10,000 messages directly (this AC is about send latency against
    // a warm table, not seeding throughput), same "seed directly"
    // rationale as agentdir_ac08_lookup_latency.rs.
    const N: usize = 10_000;
    let db = Db::open(&data_dir).expect("open db for seeding");
    db.migrate().await.expect("migrate");
    let seed_sender_ns = generate_namespace();
    let seed_sender = db
        .create_tenant("Seed Sender".to_string(), seed_sender_ns, hash_key(&generate_key()), None)
        .await
        .expect("create seed sender");
    let recipient_ns = generate_namespace();
    db.create_tenant("Seed Recipient".to_string(), recipient_ns.clone(), hash_key(&generate_key()), None)
        .await
        .expect("create seed recipient");
    for i in 0..N {
        db.msg_send(
            seed_sender.clone(),
            None,
            vec![recipient_ns.clone()],
            None,
            format!("seed {i}"),
            None,
            None,
            i64::MAX,
        )
        .await
        .expect("seed message");
    }
    // A distinct, freshly-created sender for the 200 timed live calls
    // below: `msgs_per_hour`'s sliding-window count is per-sender, and the
    // 10,000 just-seeded messages above would otherwise trip it on the
    // very first live call if this test reused `seed_sender`'s key.
    let live_sender_key = generate_key();
    let live_sender_ns = generate_namespace();
    db.create_tenant("Live Sender".to_string(), live_sender_ns.clone(), hash_key(&live_sender_key), None)
        .await
        .expect("create live sender");
    drop(db);

    let port = free_port();
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let mut child = Command::new(bin)
        .arg("serve")
        .env("MCPHOST_DATA_DIR", &data_dir)
        .env("MCPHOST_BIND", format!("127.0.0.1:{port}"))
        .env("MCPHOST_ADMIN_KEY", "msg-ac12-admin-key")
        .env("MCPHOST_SECRET_KEY", "msg-ac12-secret-key")
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

    // The free plan's msgs_per_hour (60) is well under 200; bump the live
    // sender to pro (2,000/hour) so this AC's 200 calls measure send
    // latency, not the quota AC9 already covers separately.
    let plan_set_body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "admin.plan_set",
            "arguments": {"tenant": live_sender_ns, "plan": "pro"},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
            },
        },
    });
    http.post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "admin.plan_set")
        .header("Authorization", "Bearer msg-ac12-admin-key")
        .json(&plan_set_body)
        .send()
        .await
        .expect("admin.plan_set request")
        .error_for_status()
        .expect("admin.plan_set must succeed");

    let mut durations = Vec::with_capacity(200);
    for n in 0..200 {
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {
                "name": "host.msg.send",
                "arguments": {"to": [recipient_ns], "body": format!("warm {n}")},
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
            .header("Mcp-Name", "host.msg.send")
            .header("Authorization", format!("Bearer {live_sender_key}"))
            .json(&body)
            .send()
            .await
            .expect("send request");
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert!(resp.status().is_success(), "send {n} failed: {}", resp.status());
        durations.push(elapsed_ms);
    }

    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = percentile(&durations, 0.95);
    println!("AC12 host.msg.send latency: 200 sequential calls warm, p95={p95:.2}ms");
    assert!(p95 < 100.0, "p95 {p95:.2}ms exceeds the 100ms target");

    let _ = std::fs::remove_dir_all(&data_dir);
}
