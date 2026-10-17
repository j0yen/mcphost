//! PRD-mcphost-event-kinds-registry-generated-b AC3 — Given `GET /events.json` on a fresh server,
//! When fetched twice, Then both bodies are byte-identical, `queue_states` lists
//! `active, paused, draining, drained` in order, and the response carries
//! `cache-control: public, max-age=300`.

use crate::common;
use common::TestServer;

#[tokio::test]
async fn evkrb_ac03() {
    let server = TestServer::start().await;
    let url = format!("{}/events.json", server.base_url);
    let first = reqwest::get(&url).await.expect("first GET");
    assert_eq!(first.status(), 200);
    assert_eq!(first.headers()["cache-control"], "public, max-age=300");
    let first = first.bytes().await.expect("first body");
    let second = reqwest::get(&url).await.expect("second GET").bytes().await.expect("second body");
    assert_eq!(first, second, "bodies must be byte-identical");

    let body: serde_json::Value = serde_json::from_slice(&first).expect("json");
    let states: Vec<&str> = body["queue_states"].as_array().unwrap().iter().map(|s| s["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["active", "paused", "draining", "drained"]);
}
