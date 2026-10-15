//! PRD-mcphost-task-handoff-tool AC1 (P0) — Given tenants A and B, When A
//! posts a task to B, B claims and completes it with `result: {"n": 1}`,
//! Then A's `host.msg.wait` (cursor from before the post) returns within 2 s
//! a message whose `data.task` has `status: "done"` and `result.n == 1`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn the_posters_wait_returns_the_done_message_with_the_result() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Thand A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Thand B").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);

    // A has no message yet, so "the cursor from before the post" is the
    // start of A's inbox: omitted.
    let posted = extract_structured(
        &a.tools_call("host.task.post", json!({"to": ns_b, "payload": {"job": "sum"}, "deadline_s": 600}))
            .await
            .expect("post"),
    );
    assert_eq!(posted["status"], json!("posted"), "{posted:?}");
    let task_id = posted["task_id"].as_str().expect("task_id").to_string();
    let thread_id = posted["thread_id"].as_str().expect("thread_id").to_string();

    // B's wait returns the task message carrying data.task.task_id.
    let b_wait = extract_structured(&b.tools_call("host.msg.wait", json!({"timeout_s": 2})).await.expect("B wait"));
    let first = &b_wait["messages"][0];
    assert_eq!(first["data"]["task"]["task_id"], json!(task_id), "{b_wait:?}");
    assert_eq!(first["data"]["payload"]["job"], json!("sum"), "{b_wait:?}");
    assert_eq!(first["thread_id"], json!(thread_id), "{b_wait:?}");

    let claimed = extract_structured(&b.tools_call("host.task.claim", json!({"task_id": task_id})).await.expect("claim"));
    assert_eq!(claimed["status"], json!("claimed"), "{claimed:?}");
    let done = extract_structured(
        &b.tools_call("host.task.complete", json!({"task_id": task_id, "result": {"n": 1}}))
            .await
            .expect("complete"),
    );
    assert_eq!(done["status"], json!("done"), "{done:?}");

    let mut a_wait = json!(null);
    crate::deadline_budget!(2000, {
        a_wait = extract_structured(&a.tools_call("host.msg.wait", json!({"timeout_s": 2})).await.expect("A wait"));
    });
    let done_msg = a_wait["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["data"]["task"]["status"] == json!("done"))
        .unwrap_or_else(|| panic!("no done message in {a_wait:?}"));
    assert_eq!(done_msg["data"]["task"]["task_id"], json!(task_id), "{done_msg:?}");
    assert_eq!(done_msg["data"]["task"]["result"]["n"], json!(1), "{done_msg:?}");
}
