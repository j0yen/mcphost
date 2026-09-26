//! PRD-mcphost-status-feed AC1 (P0): given the host has run for 3 minutes, an
//! anonymous `GET /status.json` returns `state: "operational"`, four
//! components each with a `last_sample` within 90s, and
//! `Cache-Control: max-age=60`.

use crate::common;

use std::time::{SystemTime, UNIX_EPOCH};

use common::{TempDataDir, TestServer, python_kind_registry};

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[tokio::test]
async fn operational_after_three_self_sample_ticks() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    // "the host has run for 3 minutes" -- three deterministic minute
    // ticks, not a real 3-minute wait (same convention every other
    // background-tick AC in this crate uses).
    for _ in 0..3 {
        mcphost::statusfeed::tick_once(&server.state)
            .await
            .expect("self-sample tick");
    }

    let resp = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json");
    assert_eq!(resp.status(), 200);
    let cache_control = resp
        .headers()
        .get("cache-control")
        .expect("Cache-Control header")
        .to_str()
        .expect("ascii header");
    assert_eq!(cache_control, "max-age=60");

    let body: serde_json::Value = resp.json().await.expect("parse /status.json");
    assert_eq!(body["state"], "operational", "body: {body}");

    let components = body["components"].as_array().expect("components array");
    assert_eq!(components.len(), 4, "body: {body}");
    let now = now_unix();
    for component in components {
        let last_sample = component.get("last_sample").expect("last_sample present");
        assert_ne!(*last_sample, serde_json::Value::Null, "component: {component}");
        let ts = last_sample["ts"].as_i64().expect("last_sample.ts");
        assert!(
            now - ts <= 90,
            "component {} last_sample.ts {} is not within 90s of {}",
            component["name"],
            ts,
            now
        );
        assert_eq!(component["state"], "ok", "component: {component}");
    }
}
