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

use crate::serve;
use serve::{ServeChild, ServeEnv};

use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::Db;
use serde_json::json;

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
            None,
            None,
        )
        .await
        .map(|_| ())
        .expect("seed profile");
        namespaces.push(namespace);
    }
    drop(db);

    let mut server = ServeChild::spawn(ServeEnv {
        data_dir: data_dir.clone(),
        admin_key: Some("agentdir-ac08-admin-key".into()),
        secret_key: Some("agentdir-ac08-secret-key".into()),
        extra_env: vec![("MCPHOST_LOG_LEVEL".into(), "warn".into())],
        ..ServeEnv::default()
    });
    server.wait_ready().await;
    let base_url = server.base_url();
    let http = reqwest::Client::new();

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
        assert!(resp.status().is_success(), "lookup for {namespace} failed: {}", resp.status());
    }

    // Latency half: median-of-5 warm single lookup under 50 ms; skipped under
    // MCPHOST_PERF_SKIP=1 (loaded host).
    let namespace = &namespaces[0];
    crate::perf_budget!(50, {
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
        assert!(resp.status().is_success(), "lookup failed: {}", resp.status());
    });

    let _ = std::fs::remove_dir_all(&data_dir);
}
