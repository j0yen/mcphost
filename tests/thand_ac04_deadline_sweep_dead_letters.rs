//! PRD-mcphost-task-handoff-tool AC4 (P0) — Given `deadline_s: 60` and no
//! claim, When the housekeeping tick runs at deadline + 60 s, Then the task
//! is `dead`, A's `dead_letters` lists it with `reason: "deadline"`, and the
//! thread's last message is from `host` with `data.task.status: "dead"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn an_unclaimed_task_past_its_deadline_is_dead_lettered_with_a_host_message() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Thand4 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Thand4 B").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);

    let posted = extract_structured(
        &a.tools_call("host.task.post", json!({"to": ns_b, "payload": {"x": 1}, "deadline_s": 60}))
            .await
            .expect("post"),
    );
    let task_id = posted["task_id"].as_str().expect("task_id").to_string();
    let thread_id = posted["thread_id"].as_str().expect("thread_id").to_string();
    let deadline = posted["deadline_unix"].as_i64().expect("deadline_unix");

    // Before the deadline a tick leaves it alone.
    let swept = mcphost::tasks::sweep_at(&server.state, deadline - 1).await.expect("early tick");
    assert_eq!(swept, 0);
    let before = extract_structured(&a.tools_call("host.task.dead_letters", json!({})).await.expect("dead_letters"));
    assert_eq!(before["dead_letters"].as_array().expect("array").len(), 0, "{before:?}");

    let swept = mcphost::tasks::sweep_at(&server.state, deadline + 60).await.expect("tick at deadline + 60 s");
    assert_eq!(swept, 1);
    // A second tick has nothing left to do.
    assert_eq!(mcphost::tasks::sweep_at(&server.state, deadline + 120).await.expect("tick"), 0);

    let got = extract_structured(&a.tools_call("host.task.get", json!({"task_id": task_id})).await.expect("get"));
    assert_eq!(got["status"], json!("dead"), "{got:?}");

    let dead = extract_structured(&a.tools_call("host.task.dead_letters", json!({})).await.expect("dead_letters"));
    let letters = dead["dead_letters"].as_array().expect("array");
    assert_eq!(letters.len(), 1, "{dead:?}");
    assert_eq!(letters[0]["task_id"], json!(task_id), "{dead:?}");
    assert_eq!(letters[0]["status"], json!("dead"), "{dead:?}");
    assert_eq!(letters[0]["reason"], json!("deadline"), "{dead:?}");

    // The thread's last message is from host, and both parties are told.
    let thread = extract_structured(
        &a.tools_call("host.msg.thread", json!({"thread_id": thread_id})).await.expect("thread"),
    );
    let messages = thread["messages"].as_array().expect("messages");
    let last = messages.last().expect("last message");
    assert_eq!(last["from_address"], json!("host"), "{thread:?}");
    assert_eq!(last["data"]["task"]["status"], json!("dead"), "{thread:?}");
    assert_eq!(last["data"]["task"]["task_id"], json!(task_id), "{thread:?}");

    let b_inbox = extract_structured(&b.tools_call("host.msg.inbox", json!({})).await.expect("B inbox"));
    assert!(
        b_inbox["messages"].as_array().expect("messages").iter().any(|m| m["data"]["task"]["status"] == json!("dead")),
        "the recipient is told too: {b_inbox:?}"
    );

    // A dead task refuses a late claim.
    let late = b.tools_call("host.task.claim", json!({"task_id": task_id})).await.expect_err("late claim");
    assert_eq!(late.error_code.as_deref(), Some("task_closed"), "{late:?}");
}
