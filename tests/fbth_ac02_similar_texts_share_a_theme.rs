//! PRD-mcphost-feedback-themes-nightly AC2 — Given two rows with texts
//! `args_invalid said 4.78 is not an integer` and `args_invalid says 12.5 is
//! not an integer` on the same tool, When clustered, Then they share one
//! `theme_id` with `count: 2` and both ids in `example_ids`.

use crate::common;
use common::TestServer;
use mcphost::db::FeedbackCap;

async fn file(server: &TestServer, id: &str, text: &str, at: i64) {
    server
        .state
        .db
        .insert_feedback(
            id.to_string(),
            None,
            "wrong_error".to_string(),
            text.to_string(),
            Some(r#"{"error_code":"args_invalid","tool":"host.tool_call"}"#.to_string()),
            None,
            None,
            r#"["c_1","c_2"]"#.to_string(),
            "iphash".to_string(),
            at,
            FeedbackCap::None,
        )
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn two_numeric_variants_land_in_one_theme() {
    let server = TestServer::start().await;
    file(&server, "f_one", "args_invalid said 4.78 is not an integer", 1_000).await;
    file(&server, "f_two", "args_invalid says 12.5 is not an integer", 2_000).await;

    let stats = server.state.db.cluster_feedback().await.unwrap();
    assert_eq!(stats.rows_clustered, 2);

    let themes = server.state.db.list_feedback_themes(0, None, None, 100).await.unwrap();
    assert_eq!(themes.len(), 1, "one theme: {themes:?}");
    let t = &themes[0];
    assert_eq!(t.count, 2);
    assert!(t.theme_id.starts_with("th_") && t.theme_id.len() == 15, "{}", t.theme_id);
    assert_eq!(t.example_ids, vec!["f_one", "f_two"], "oldest first");
    assert_eq!((t.first_seen_unix, t.last_seen_unix), (1_000, 2_000));
    assert_eq!(t.call_ids, vec!["c_1", "c_2"], "union of member call ids, deduplicated");
    assert_eq!(t.status, "open");
    for id in ["f_one", "f_two"] {
        assert_eq!(
            server.state.db.feedback_row_theme_id(id.to_string()).await.unwrap().as_deref(),
            Some(t.theme_id.as_str())
        );
    }
}
