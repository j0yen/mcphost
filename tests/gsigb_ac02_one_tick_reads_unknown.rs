//! PRD-mcphost-golden-signals-self-probe-b AC2 (P0): after a fresh boot and
//! exactly one tick, every signal reads `value: null, band: "unknown"` and
//! `/status.json` is still HTTP 200.

use crate::common;

use common::TestServer;

#[tokio::test]
async fn one_tick_leaves_every_signal_null_and_unknown_with_status_200() {
    let server = TestServer::start().await;
    mcphost::statusfeed::tick_once(&server.state).await.expect("tick");

    let resp = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json");
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("parse body");

    let signals = body["signals"].as_object().expect("signals object");
    assert_eq!(signals.len(), mcphost::signals::Signal::ALL.len(), "body: {body}");
    for signal in mcphost::signals::Signal::ALL {
        let s = &signals[signal.name()];
        assert!(s["value"].is_null(), "{} value: {s}", signal.name());
        assert_eq!(s["band"], "unknown", "{} band: {s}", signal.name());
    }
}
