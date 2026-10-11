//! PRD-mcphost-status-host-pressure AC4 (P0): an anonymous
//! `GET /status.json` carries `host.sampled_at` within 5 s of now, and the
//! pre-existing keys are unchanged.

use crate::common;

use common::TestServer;
use mcphost::state::now_unix;

#[tokio::test]
async fn status_json_has_host_and_keeps_existing_keys() {
    let server = TestServer::start().await;
    let resp = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("cache-control").and_then(|v| v.to_str().ok()),
        Some("max-age=60")
    );
    let body: serde_json::Value = resp.json().await.expect("parse body");

    let sampled_at = body["host"]["sampled_at"].as_i64().expect("host.sampled_at: {body}");
    assert!((now_unix() - sampled_at).abs() <= 5, "sampled_at {sampled_at} vs now {}", now_unix());
    for k in ["load1", "load5", "psi_cpu_some_avg60", "cpu_steal_pct_since_boot", "mem_available_mb", "nproc"] {
        assert!(body["host"].get(k).is_some(), "host.{k} key present (null allowed): {body}");
    }

    assert!(body["state"].is_string());
    assert!(body["generated_at"].is_i64());
    assert_eq!(body["components"].as_array().expect("components").len(), 4);
    assert!(body["incidents_open"].is_array());
    assert!(body["incidents_recent_30d"].is_array());
    assert!(body["warnings"].is_array());
}
