//! PRD-mcphost-task-handoff-tool AC7 (P0) — Given tenant B is deleted, When
//! `host.task.get` and `host.msg.thread` are called by A, Then the task row
//! is gone and the thread no longer lists B as a participant.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn deleting_the_recipient_removes_the_task_and_its_participation() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Thand7 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Thand7 B").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let posted = extract_structured(
        &a.tools_call("host.task.post", json!({"to": ns_b, "payload": {"x": 1}, "deadline_s": 600}))
            .await
            .expect("post"),
    );
    let task_id = posted["task_id"].as_str().expect("task_id").to_string();
    let thread_id = posted["thread_id"].as_str().expect("thread_id").to_string();
    b.tools_call("host.task.claim", json!({"task_id": task_id})).await.expect("claim");
    assert_eq!(
        server.state.db.count_thread_participants(thread_id.clone()).await.expect("participants"),
        2,
        "A and B before the delete"
    );

    admin.tools_call("admin.tenant_delete", json!({"tenant": ns_b})).await.expect("admin.tenant_delete");

    let gone = a.tools_call("host.task.get", json!({"task_id": task_id})).await.expect_err("task is gone");
    assert_eq!(gone.error_code.as_deref(), Some("task_not_found"), "{gone:?}");

    // The thread survives for A, with B no longer a participant.
    let thread = extract_structured(
        &a.tools_call("host.msg.thread", json!({"thread_id": thread_id})).await.expect("thread"),
    );
    assert!(!thread["messages"].as_array().expect("messages").is_empty(), "A keeps the history: {thread:?}");
    assert_eq!(
        server.state.db.count_thread_participants(thread_id.clone()).await.expect("participants"),
        1,
        "only A is left"
    );

    // Nothing of the task lingers for the poster either.
    let dead = extract_structured(&a.tools_call("host.task.dead_letters", json!({})).await.expect("dead_letters"));
    assert_eq!(dead["dead_letters"].as_array().expect("array").len(), 0, "{dead:?}");
    assert_eq!(mcphost::tasks::sweep_at(&server.state, i64::MAX / 2).await.expect("tick"), 0);
}
