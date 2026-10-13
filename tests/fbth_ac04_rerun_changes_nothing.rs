//! PRD-mcphost-feedback-themes-nightly AC4 — Given a clustered table, When
//! the nightly task runs again with no new rows, Then no theme row changes
//! (`count`, `last_seen`, `example_ids` identical) and 0 `feedback` rows are
//! updated.

use crate::common;
use common::TestServer;
use mcphost::db::FeedbackCap;
use std::time::Duration;

async fn file(server: &TestServer, id: &str, text: &str, at: i64) {
    server
        .state
        .db
        .insert_feedback(
            id.to_string(),
            None,
            "bug".to_string(),
            text.to_string(),
            Some(r#"{"error_code":"internal","tool":"host.tool_call"}"#.to_string()),
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
async fn second_pass_with_no_new_rows_updates_nothing() {
    let server = TestServer::start().await;
    for i in 0..6 {
        file(&server, &format!("f_{i}"), &format!("internal error request {i} failed"), 1_000 + i).await;
    }
    file(&server, "f_other", "something unrelated broke", 2_000).await;

    let first = server.state.db.cluster_feedback().await.unwrap();
    assert_eq!(first.rows_clustered, 7);
    let before = server.state.db.list_feedback_themes(0, None, None, 100).await.unwrap();
    assert_eq!(before.len(), 2);
    assert_eq!(before.iter().map(|t| t.count).max(), Some(6));
    assert_eq!(before.iter().map(|t| t.example_ids.len()).max(), Some(5), "examples capped at 5");

    let second = server.state.db.cluster_feedback().await.unwrap();
    assert_eq!(second.rows_clustered, 0, "0 feedback rows updated");
    assert_eq!(second.themes_touched, 0);
    let after = server.state.db.list_feedback_themes(0, None, None, 100).await.unwrap();
    assert_eq!(before, after, "count, last_seen, example_ids all identical");

    // The unattended loop is the same pass: a new row is picked up, old
    // themes are untouched.
    file(&server, "f_new", "something unrelated broke", 3_000).await;
    let handle = mcphost::feedback::spawn_theme_scheduler_for_test((*server.state).clone(), Duration::from_millis(50));
    for _ in 0..100 {
        if server.state.db.feedback_row_theme_id("f_new".into()).await.unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    handle.abort();
    let grown = server.state.db.list_feedback_themes(0, None, None, 100).await.unwrap();
    let other = grown.iter().find(|t| t.example_ids.contains(&"f_new".to_string())).expect("new row clustered");
    assert_eq!(other.count, 2);
    assert_eq!(other.last_seen_unix, 3_000);
}

#[test]
fn next_run_is_0310_utc() {
    // 1970-01-01T00:00:00Z -> 03:10 the same day; at 03:10:00 -> next day.
    assert_eq!(mcphost::feedback::secs_until_next_theme_run(0), 3 * 3600 + 600);
    assert_eq!(mcphost::feedback::secs_until_next_theme_run(3 * 3600 + 600), 86_400);
}
