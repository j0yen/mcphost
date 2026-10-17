//! PRD-mcphost-msg-wait-long-poll-contract-b
//! AC1 (P0) — Given `timeout_s: 90`, When either wait verb runs, Then
//! `timeout_s_applied: 25` and the call never exceeds 26 s; Given
//! `timeout_s: "soon"`, Then `args_invalid`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn timeout_above_ceiling_is_clamped_and_non_integer_is_args_invalid() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "mwlpb ac01 tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.tool_publish", json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    let enqueue = extract_structured(
        &client.tools_call("host.tool_call", json!({"name": "echoer", "args": {}, "async": true})).await.expect("enqueue"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    // runs.wait: the run finalizes, so this returns early but still reports the clamp.
    let runs = crate::deadline_budget!(26_000, {
        extract_structured(&client.tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": 90})).await.expect("runs.wait"))
    });
    assert_eq!(runs["timeout_s_applied"], json!(25), "{runs}");

    // msg.wait on an empty inbox runs the full clamped window.
    let msg = crate::deadline_budget!(26_000, {
        extract_structured(&client.tools_call("host.msg.wait", json!({"timeout_s": 90})).await.expect("msg.wait"))
    });
    assert_eq!(msg["timeout_s_applied"], json!(25), "{msg}");
    assert_eq!(msg["timed_out"], json!(true), "{msg}");

    let err = client.tools_call("host.msg.wait", json!({"timeout_s": "soon"})).await.expect_err("string timeout_s");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    let err = client
        .tools_call("host.runs.wait", json!({"run_id": run_id, "timeout_s": "soon"}))
        .await
        .expect_err("string timeout_s");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
}
