//! PRD-mcphost-golden-signals-self-probe AC2 (P0): after 3 probe ticks
//! `/status.json` carries all five signals, each with a numeric `value`, a
//! band in {green, amber, red}, and at least 3 echo samples in the window.

use crate::common;

use common::TestServer;

#[tokio::test]
async fn status_json_has_five_graded_signals_after_three_ticks() {
    let server = TestServer::start().await;
    for _ in 0..3 {
        mcphost::statusfeed::tick_once(&server.state).await.expect("tick");
    }

    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse body");

    let signals = body["signals"].as_object().expect("signals object");
    assert_eq!(signals.len(), 5, "body: {body}");
    for key in ["echo_p95_ms", "cold_start_p50_ms", "sqlite_write_ms", "disk_free_mb", "sandbox_pool"] {
        let s = &signals[key];
        assert!(s["value"].is_number(), "{key} value: {s}");
        let band = s["band"].as_str().expect("band string");
        assert!(["green", "amber", "red"].contains(&band), "{key} band: {s}");
        assert!(s["amber"].is_number() && s["red"].is_number() && s["unit"].is_string(), "{key}: {s}");
    }
    let echo = &signals["echo_p95_ms"];
    assert!(echo["samples_5m"].as_u64().expect("samples_5m") >= 3, "echo: {echo}");
}
