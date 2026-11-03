//! AC8 (PRD-mcphost-admin-signup-pause-tool) — Given `message: "window"` and
//! `until` 600 s ahead, When paused, Then the file's line 2 is `until=<ts>`,
//! the result and `signup`'s error carry `retry_after_secs` within 2 s of 600,
//! and alert `signup.paused` is raised within `PAUSE_POLL_INTERVAL_SECS` + 2 s
//! and `signup.resumed` after `resume: true`.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;
use std::time::{Duration, Instant};

async fn wait_for_alert(server: &TestServer, key: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(mcphost::alerts::PAUSE_POLL_INTERVAL_SECS + 2);
    loop {
        if let Ok(Some(_)) = server.state.db.most_recent_alert_for_key(key.to_string()).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn aspt_ac08_until_line_bounds_retry_after_and_both_alerts_fire() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let until = mcphost::state::now_unix() + 600;

    let result = admin
        .tools_call("admin.signup_pause", json!({"message": "window", "until": until}))
        .await
        .expect("admin.signup_pause with until");
    let out = extract_structured(&result);
    assert_eq!(out["paused"], json!(true), "{out:?}");
    assert_eq!(out["until"], json!(until), "{out:?}");
    let retry = out["retry_after_secs"].as_i64().expect("retry_after_secs");
    assert!((598..=600).contains(&retry), "retry_after_secs {retry} not within 2 s of 600: {out:?}");

    let file = std::fs::read_to_string(server.state.signup_pause.path()).expect("pause file");
    let mut lines = file.lines();
    assert_eq!(lines.next(), Some("window"));
    assert_eq!(lines.next(), Some(format!("until={until}").as_str()), "{file:?}");

    let err = McpClient::new(&server.base_url)
        .tools_call("signup", json!({"name": "During Window"}))
        .await
        .expect_err("signup during the window must fail");
    assert_eq!(err.error_code.as_deref(), Some("signup_paused"), "{err:?}");
    let signup_retry = err.data["retry_after_secs"].as_i64().expect("signup retry_after_secs");
    assert!((598..=600).contains(&signup_retry), "signup retry_after_secs {signup_retry}: {err:?}");

    assert!(
        wait_for_alert(&server, "signup.paused").await,
        "signup.paused not raised within PAUSE_POLL_INTERVAL_SECS + 2 s"
    );

    admin
        .tools_call("admin.signup_pause", json!({"resume": true}))
        .await
        .expect("resume");
    assert!(
        wait_for_alert(&server, "signup.resumed").await,
        "signup.resumed not raised within PAUSE_POLL_INTERVAL_SECS + 2 s of resume"
    );
}
