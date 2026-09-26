//! PRD-mcphost-status-feed AC2 (P0): given the sandbox executor is disabled in
//! the test harness (the default `TestServer` registers no `python` kind),
//! when 5 sample ticks pass, then `components[exec].state` is `"failing"`
//! and the overall `state` is `"degraded"`.

use crate::common;

use common::TestServer;

#[tokio::test]
async fn exec_disabled_degrades_overall_state() {
    let server = TestServer::start().await;

    for _ in 0..5 {
        mcphost::statusfeed::tick_once(&server.state)
            .await
            .expect("self-sample tick");
    }

    let body = mcphost::statusfeed::status_json(&server.state)
        .await
        .expect("status_json");
    assert_eq!(body["state"], "degraded", "body: {body}");

    let components = body["components"].as_array().expect("components array");
    let exec = components
        .iter()
        .find(|c| c["name"] == "exec")
        .expect("exec component present");
    assert_eq!(exec["state"], "failing", "exec: {exec}");

    for component in components {
        if component["name"] != "exec" {
            assert_eq!(component["state"], "ok", "component: {component}");
        }
    }
}
