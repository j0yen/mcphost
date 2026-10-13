//! PRD-mcphost-feedback-themes-nightly AC3 — Given rows with the same text
//! but different `error_code`, When clustered, Then they land in different
//! themes.

use crate::common;
use common::TestServer;
use mcphost::db::FeedbackCap;

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
async fn same_text_different_error_code_is_two_themes() {
    let server = TestServer::start().await;
    file(&server, "f_a", "args_invalid", 1_000).await;
    file(&server, "f_b", "invalid_spec", 1_001).await;
    file(&server, "f_c", "invalid_spec", 1_002).await;

    server.state.db.cluster_feedback().await.unwrap();

    let a = server.state.db.feedback_row_theme_id("f_a".into()).await.unwrap().expect("a clustered");
    let b = server.state.db.feedback_row_theme_id("f_b".into()).await.unwrap().expect("b clustered");
    let c = server.state.db.feedback_row_theme_id("f_c".into()).await.unwrap().expect("c clustered");
    assert_ne!(a, b, "different error_code, different theme");
    assert_eq!(b, c, "same error_code and text, same theme");
    let themes = server.state.db.list_feedback_themes(0, None, None, 100).await.unwrap();
    assert_eq!(themes.len(), 2);
    assert_eq!(themes[0].count, 2, "sorted count desc");
}
