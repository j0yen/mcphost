//! PRD-mcphost-task-handoff-tool AC6 (P0) — Given a free tenant with 20
//! open tasks, When it posts a 21st, Then `quota_exceeded` names
//! `tasks_open_max`; Given a `to` address that blocked A, Then
//! `agent_not_found`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn the_21st_open_task_hits_tasks_open_max_and_a_blocker_is_agent_not_found() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "Thand6 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Thand6 B").await;
    let (ns_c, key_c) = signup(&server.base_url, "Thand6 C").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let c = McpClient::with_bearer(&server.base_url, &key_c);

    let mut first_id = String::new();
    for i in 0..20 {
        let posted = extract_structured(
            &a.tools_call("host.task.post", json!({"to": ns_b, "payload": {"i": i}, "deadline_s": 600}))
                .await
                .unwrap_or_else(|e| panic!("post {i}: {e:?}")),
        );
        if i == 0 {
            first_id = posted["task_id"].as_str().expect("task_id").to_string();
        }
    }
    let over = a
        .tools_call("host.task.post", json!({"to": ns_b, "payload": {"i": 20}, "deadline_s": 600}))
        .await
        .expect_err("21st post");
    assert_eq!(over.error_code.as_deref(), Some("quota_exceeded"), "{over:?}");
    assert_eq!(over.data["limit"], json!("tasks_open_max"), "{over:?}");
    assert_eq!(over.data["value"], json!(20), "{over:?}");

    // Finishing one frees a slot: the quota counts only posted|claimed.
    b.tools_call("host.task.claim", json!({"task_id": first_id})).await.expect("claim");
    b.tools_call("host.task.complete", json!({"task_id": first_id, "result": {}})).await.expect("complete");
    let again = extract_structured(
        &a.tools_call("host.task.post", json!({"to": ns_b, "payload": {"i": 21}, "deadline_s": 600}))
            .await
            .expect("a slot freed by the completion"),
    );
    // Free one slot again so the block below, not the quota, decides.
    let again_id = again["task_id"].as_str().expect("task_id");
    b.tools_call("host.task.claim", json!({"task_id": again_id})).await.expect("claim");
    b.tools_call("host.task.complete", json!({"task_id": again_id, "result": {}})).await.expect("complete");

    // C blocks A: a task to C is agent_not_found, byte-identical to an
    // address nobody holds, and nothing is stored for it.
    c.tools_call("host.msg.block", json!({"address": ns_a})).await.expect("C blocks A");
    let blocked = c_post(&a, &ns_c).await;
    assert_eq!(blocked.error_code.as_deref(), Some("agent_not_found"), "{blocked:?}");
    let nobody = c_post(&a, "t_nobody_here").await;
    assert_eq!(blocked.message, nobody.message, "a blocker looks like an address nobody holds");
}

async fn c_post(a: &McpClient, to: &str) -> common::RpcError {
    a.tools_call("host.task.post", json!({"to": to, "payload": {"x": 1}, "deadline_s": 600}))
        .await
        .expect_err("refused post")
}
