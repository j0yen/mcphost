//! PRD-mcphost-reachability-alt-host
//! AC3 (P0) — Given any public host, When `GET /reach` is called with no
//! auth, Then `200 {host, ok: true, served_at}` and no row is written
//! anywhere.

use crate::common;
use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn reach_is_unauthenticated_and_writes_nothing() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    // Admin-only counts before, to prove no row got written by /reach.
    let admin_before = http
        .get(format!("{}/healthz", server.base_url))
        .header("Authorization", "Bearer test-admin-key-not-for-production")
        .send()
        .await
        .expect("admin healthz before")
        .json::<serde_json::Value>()
        .await
        .expect("json");

    // No Authorization header at all.
    let resp = http
        .get(format!("{}/reach", server.base_url))
        .header("Host", "alt.example")
        .send()
        .await
        .expect("GET /reach");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let before = mcphost::state::now_unix();
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["host"], json!("alt.example"));
    assert_eq!(body["ok"], json!(true));
    let served_at = body["served_at"].as_i64().expect("served_at is a unix timestamp");
    assert!((served_at - before).abs() < 5, "served_at must be fresh: {served_at} vs {before}");
    assert_eq!(body.as_object().unwrap().len(), 3, "exactly {{host, ok, served_at}}: {body}");

    // Hit it a few more times, then prove the admin-visible counts
    // (tenants_total/tools_total/signups) are byte-for-byte unchanged --
    // /reach never writes a row.
    for _ in 0..3 {
        http.get(format!("{}/reach", server.base_url))
            .send()
            .await
            .expect("GET /reach again");
    }
    let admin_after = http
        .get(format!("{}/healthz", server.base_url))
        .header("Authorization", "Bearer test-admin-key-not-for-production")
        .send()
        .await
        .expect("admin healthz after")
        .json::<serde_json::Value>()
        .await
        .expect("json");
    assert_eq!(admin_before["tenants_total"], admin_after["tenants_total"]);
    assert_eq!(admin_before["tools_total"], admin_after["tools_total"]);
    assert_eq!(admin_before["signups"], admin_after["signups"]);
}
