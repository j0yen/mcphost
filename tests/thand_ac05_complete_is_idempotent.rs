//! PRD-mcphost-task-handoff-tool AC5 (P0) — Given a completed task, When
//! `complete` is called again with a different result, Then the stored
//! result is returned with `already_complete: true` and no new thread
//! message is posted.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

async fn thread_len(c: &McpClient, thread_id: &str) -> usize {
    let t = extract_structured(&c.tools_call("host.msg.thread", json!({"thread_id": thread_id})).await.expect("thread"));
    t["messages"].as_array().expect("messages").len()
}

#[tokio::test]
async fn a_repeat_complete_returns_the_stored_result_and_posts_nothing() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Thand5 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Thand5 B").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);

    let posted = extract_structured(
        &a.tools_call("host.task.post", json!({"to": ns_b, "payload": {"x": 1}, "deadline_s": 600}))
            .await
            .expect("post"),
    );
    let task_id = posted["task_id"].as_str().expect("task_id").to_string();
    let thread_id = posted["thread_id"].as_str().expect("thread_id").to_string();

    // Completing before claiming is refused and changes nothing.
    let early = b
        .tools_call("host.task.complete", json!({"task_id": task_id, "result": {"n": 0}}))
        .await
        .expect_err("complete before claim");
    assert_eq!(early.error_code.as_deref(), Some("task_not_claimed"), "{early:?}");

    b.tools_call("host.task.claim", json!({"task_id": task_id})).await.expect("claim");
    let first = extract_structured(
        &b.tools_call("host.task.complete", json!({"task_id": task_id, "result": {"n": 1}}))
            .await
            .expect("complete"),
    );
    assert_eq!(first["already_complete"], json!(false), "{first:?}");

    let before = thread_len(&a, &thread_id).await;

    let again = extract_structured(
        &b.tools_call("host.task.complete", json!({"task_id": task_id, "result": {"n": 2}}))
            .await
            .expect("repeat complete"),
    );
    assert_eq!(again["already_complete"], json!(true), "{again:?}");
    assert_eq!(again["result"], json!({"n": 1}), "the stored result, not the new one: {again:?}");
    assert_eq!(again["status"], json!("done"), "{again:?}");

    assert_eq!(thread_len(&a, &thread_id).await, before, "no new thread message");
    let got = extract_structured(&a.tools_call("host.task.get", json!({"task_id": task_id})).await.expect("get"));
    assert_eq!(got["result"], json!({"n": 1}), "{got:?}");
}
