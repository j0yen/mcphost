//! PRD-mcphost-feedback-themes-nightly AC6 — Given `/status.json`, When
//! parsed, Then `feedback` has exactly the keys `rows_24h` and
//! `themes_open`, both integers (and no text of any row).

use crate::common;
use common::TestServer;
use mcphost::db::FeedbackCap;

async fn status(server: &TestServer) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse body")
}

#[tokio::test]
async fn feedback_object_has_exactly_two_integer_keys() {
    let server = TestServer::start().await;
    let empty = status(&server).await;
    let fb = empty["feedback"].as_object().expect("feedback object");
    let mut keys: Vec<&str> = fb.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["rows_24h", "themes_open"]);
    assert_eq!((fb["rows_24h"].as_i64(), fb["themes_open"].as_i64()), (Some(0), Some(0)));

    let now = mcphost::state::now_unix();
    for (id, at) in [("f_new1", now - 60), ("f_new2", now - 120), ("f_old", now - 3 * 86_400)] {
        server
            .state
            .db
            .insert_feedback(
                id.into(),
                None,
                "bug".into(),
                "SECRET-TEXT-MARKER broke".into(),
                None,
                None,
                None,
                "[]".into(),
                "iphash".into(),
                at,
                FeedbackCap::None,
            )
            .await
            .unwrap()
            .unwrap();
    }
    server.state.db.cluster_feedback().await.unwrap();

    let body = status(&server).await;
    let fb = body["feedback"].as_object().expect("feedback object");
    assert_eq!(fb.len(), 2, "exactly two keys: {fb:?}");
    assert_eq!(fb["rows_24h"].as_i64(), Some(2), "the 3-day-old row is outside 24h");
    assert_eq!(fb["themes_open"].as_i64(), Some(1));
    assert!(!body.to_string().contains("SECRET-TEXT-MARKER"), "no feedback text on the public feed");
}
