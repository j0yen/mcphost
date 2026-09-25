//! PRD-mcphost-alerting-webhook
//! AC2 — Given the pause alert was raised 10 s ago, When the pause toggles
//! again inside the cooldown, Then no second POST is sent and the open
//! row's `repeat_count` is 1.

use crate::common;
use common::TestServer;
use serde_json::Value;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn wait_for_alert_delivered(server: &TestServer, key: &str) -> mcphost::db::Alert {
    for _ in 0..60 {
        if let Ok(Some(row)) = server.state.db.most_recent_alert_for_key(key.to_string()).await
            && row.delivery_status == "delivered"
        {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("alert for key {key} did not reach delivery_status: delivered within 30s");
}

#[tokio::test]
async fn a_second_pause_inside_the_cooldown_collapses_into_the_open_row() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/alerts"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;

    let alert_config = mcphost::alerts::AlertConfig {
        webhook_url: Some(format!("{}/alerts", mock.uri())),
        ..mcphost::alerts::AlertConfig::default()
    };
    let server = TestServer::start_with_alert_config(alert_config).await;

    // First pause: raises and delivers `signup.paused`.
    std::fs::write(server.state.signup_pause.path(), "maintenance\n").expect("write pause file");
    let first = wait_for_alert_delivered(&server, "signup.paused").await;

    // AC2's own precondition: the alert was raised 10s ago (well inside the
    // default 900s cooldown), without a real 10s wait.
    server
        .state
        .db
        .set_alert_raised_at_for_test(first.id, mcphost::state::now_unix() - 10)
        .await
        .expect("backdate raised_at");

    // Resume (a distinct key -- unrelated to signup.paused's own cooldown),
    // then pause again -- the second `signup.paused` raise, inside cooldown.
    std::fs::remove_file(server.state.signup_pause.path()).expect("remove pause file");
    wait_for_alert_delivered(&server, "signup.resumed").await;
    std::fs::write(server.state.signup_pause.path(), "maintenance again\n")
        .expect("re-write pause file");

    // Give the pause-watch tick (3s cadence) time to observe the second
    // transition and collapse it into the open row.
    tokio::time::sleep(Duration::from_secs(6)).await;

    let row = server
        .state
        .db
        .get_alert_for_test(first.id)
        .await
        .expect("query")
        .expect("row still exists");
    assert_eq!(row.repeat_count, 1, "the collapsed duplicate must increment repeat_count once");
    assert_eq!(row.delivery_status, "delivered", "the original row's own delivery is unaffected");

    let requests = mock.received_requests().await.expect("mock records requests");
    let paused_posts = requests
        .iter()
        .filter(|r| {
            r.body_json::<Value>()
                .map(|b| b["key"] == "signup.paused")
                .unwrap_or(false)
        })
        .count();
    assert_eq!(paused_posts, 1, "exactly one signup.paused POST, never a second inside cooldown");
}
