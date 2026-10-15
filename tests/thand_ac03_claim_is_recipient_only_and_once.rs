//! PRD-mcphost-task-handoff-tool AC3 (P0) — Given a posted task, When B and
//! a third tenant C both call `claim`, Then B gets `status: "claimed"` and C
//! gets `task_not_found`; a second `claim` by B returns
//! `task_already_claimed`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn only_the_recipient_claims_and_only_once() {
    let server = TestServer::start().await;
    let (_ns_a, key_a) = signup(&server.base_url, "Thand3 A").await;
    let (ns_b, key_b) = signup(&server.base_url, "Thand3 B").await;
    let (_ns_c, key_c) = signup(&server.base_url, "Thand3 C").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);
    let c = McpClient::with_bearer(&server.base_url, &key_c);

    let posted = extract_structured(
        &a.tools_call("host.task.post", json!({"to": ns_b, "payload": {"x": 1}, "deadline_s": 600}))
            .await
            .expect("post"),
    );
    let task_id = posted["task_id"].as_str().expect("task_id").to_string();

    // A third tenant cannot tell the task from one that never existed.
    let c_err = c.tools_call("host.task.claim", json!({"task_id": task_id})).await.expect_err("C claim");
    assert_eq!(c_err.error_code.as_deref(), Some("task_not_found"), "{c_err:?}");
    let unknown = c.tools_call("host.task.claim", json!({"task_id": "no-such-task"})).await.expect_err("unknown");
    assert_eq!(c_err.message, unknown.message, "byte-identical message");
    let (mut c_data, mut unknown_data) = (c_err.data.clone(), unknown.data.clone());
    for d in [&mut c_data, &mut unknown_data] {
        d.as_object_mut().expect("data object").remove("request_id");
    }
    assert_eq!(c_data, unknown_data, "byte-identical data (request_id aside)");

    // The poster is not the recipient either.
    let a_err = a.tools_call("host.task.claim", json!({"task_id": task_id})).await.expect_err("A claim");
    assert_eq!(a_err.error_code.as_deref(), Some("task_not_found"), "{a_err:?}");

    let claimed = extract_structured(&b.tools_call("host.task.claim", json!({"task_id": task_id})).await.expect("B claim"));
    assert_eq!(claimed["status"], json!("claimed"), "{claimed:?}");

    let again = b.tools_call("host.task.claim", json!({"task_id": task_id})).await.expect_err("second claim");
    assert_eq!(again.error_code.as_deref(), Some("task_already_claimed"), "{again:?}");

    // And C's failed attempts claimed nothing.
    let got = extract_structured(&b.tools_call("host.task.get", json!({"task_id": task_id})).await.expect("get"));
    assert_eq!(got["status"], json!("claimed"), "{got:?}");
    assert_eq!(got["role"], json!("recipient"), "{got:?}");
}
