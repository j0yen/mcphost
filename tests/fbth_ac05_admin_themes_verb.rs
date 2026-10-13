//! PRD-mcphost-feedback-themes-nightly AC5 — Given `MCPHOST_ADMIN_KEY` and 3
//! themes, When `admin.feedback.themes {since:"7d"}` is called, Then 3 themes
//! come back sorted by `count` desc and a call without the key is refused
//! (this repo's wire codes: `tenant_key_missing` with no credential,
//! `bearer_invalid` -- the renamed `AppError::Unauthorized` -- for a wrong key).

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use mcphost::db::FeedbackCap;
use serde_json::json;

async fn file(server: &TestServer, id: &str, code: &str, at: i64) {
    server
        .state
        .db
        .insert_feedback(
            id.to_string(),
            None,
            "wrong_error".to_string(),
            "the error did not say what to change".to_string(),
            Some(format!(r#"{{"error_code":"{code}","tool":"host.tool_call"}}"#)),
            None,
            None,
            "[]".to_string(),
            "iphash".to_string(),
            at,
            FeedbackCap::None,
        )
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn three_themes_sorted_by_count_and_keyless_is_refused() {
    let server = TestServer::start().await;
    let now = mcphost::state::now_unix();
    for i in 0..3 {
        file(&server, &format!("f_a{i}"), "args_invalid", now - 100).await;
    }
    for i in 0..2 {
        file(&server, &format!("f_b{i}"), "invalid_spec", now - 90).await;
    }
    file(&server, "f_c0", "call_timeout", now - 80).await;
    file(&server, "f_old", "storage", now - 40 * 86_400).await;
    server.state.db.cluster_feedback().await.unwrap();

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let res = admin.tools_call("admin.feedback.themes", json!({"since": "7d"})).await.expect("admin call");
    let out = extract_structured(&res);
    let themes = out["themes"].as_array().expect("themes array");
    assert_eq!(themes.len(), 3, "the 40-day-old theme is outside 7d: {out}");
    let counts: Vec<i64> = themes.iter().map(|t| t["count"].as_i64().unwrap()).collect();
    assert_eq!(counts, vec![3, 2, 1], "count desc");
    assert!(out["generated_unix"].as_i64().is_some());
    assert!(themes[0].get("text").is_none(), "no raw text on the wire");
    assert_eq!(themes[0]["example_ids"].as_array().unwrap().len(), 3);

    let wide = extract_structured(&admin.tools_call("admin.feedback.themes", json!({"since": "30d"})).await.unwrap());
    assert_eq!(wide["themes"].as_array().unwrap().len(), 3, "30d still excludes the 40-day-old one");

    let keyless = McpClient::new(&server.base_url);
    let err = keyless
        .tools_call("admin.feedback.themes", json!({"since": "7d"}))
        .await
        .expect_err("no key, no themes");
    assert_eq!(err.error_code.as_deref(), Some("tenant_key_missing"), "no credential at all");

    let wrong = McpClient::with_bearer(&server.base_url, "not-the-admin-key");
    let err = wrong
        .tools_call("admin.feedback.themes", json!({"since": "7d"}))
        .await
        .expect_err("a wrong key, no themes");
    assert_eq!(err.error_code.as_deref(), Some("bearer_invalid"), "AppError::Unauthorized");
}
