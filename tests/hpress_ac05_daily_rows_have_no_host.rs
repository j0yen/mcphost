//! PRD-mcphost-status-host-pressure AC5 (P0): `GET /status.json?component=
//! mcp&days=7` keeps today's daily-rows body, byte for byte, with no `host`
//! key.

use crate::common;

use common::TestServer;
use mcphost::state::{now_unix, rfc3339_from_unix};

#[tokio::test]
async fn component_days_query_body_is_the_unchanged_daily_rows() {
    let server = TestServer::start().await;
    for off in 0..7i64 {
        let day = rfc3339_from_unix(now_unix() - off * 86_400)[..10].to_string();
        server
            .state
            .db
            .upsert_status_daily("mcp".to_string(), day, 1400 + off, 1440, 20 + off)
            .await
            .expect("upsert_status_daily");
    }

    let resp = reqwest::Client::new()
        .get(format!("{}/status.json?component=mcp&days=7", server.base_url))
        .send()
        .await
        .expect("GET");
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.expect("body bytes");

    let expected = mcphost::statusfeed::daily_rows(&server.state, "mcp", 7)
        .await
        .expect("daily_rows");
    assert_eq!(bytes.as_ref(), serde_json::to_vec(&expected).unwrap().as_slice());
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v.get("host").is_none(), "no host key: {v}");
    assert_eq!(v["days"].as_array().unwrap().len(), 7);
}
