//! PRD-mcphost-form-tool-b
//! AC1 (P0) — Given an agent blocked on `host.msg.wait`, When a valid form
//! POST completes, Then the wait returns a `form_submission` message carrying
//! the same `args` and `run_id` within 2 s, and a replayed POST with the same
//! CSRF token does not produce a second message.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::time::Duration;

fn csrf_of(html: &str) -> String {
    html.split("name=\"_csrf\" value=\"").nth(1).and_then(|r| r.split('"').next()).expect("hidden csrf field").to_string()
}

#[tokio::test]
async fn valid_post_wakes_blocked_wait_within_two_seconds_and_replay_adds_no_message() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Ftoolb AC1").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ask_amount", "kind": "echo",
                   "spec": {"schema": {"type": "object", "properties": {"note": {"type": "string"}}, "required": ["note"]},
                            "ui": {"form": true}}}),
        )
        .await
        .expect("publish");
    let shared = extract_structured(
        &client.tools_call("host.tool_share", json!({"name": "ask_amount", "visibility": "url"})).await.expect("share"),
    );
    let form_url = shared["form_url"].as_str().expect("form_url").to_string();

    let http = reqwest::Client::new();
    let csrf = csrf_of(&http.get(&form_url).send().await.unwrap().text().await.unwrap());

    // Block in host.msg.wait first, then submit.
    let waiter_client = McpClient::with_bearer(&server.base_url, &key);
    let waiter = tokio::spawn(async move {
        // Budget = the 500 ms head start below + the 2 s allowed after the POST.
        crate::deadline_budget!(2500, {
            let out = waiter_client.tools_call("host.msg.wait", json!({"timeout_s": 20})).await.expect("wait");
            extract_structured(&out)
        })
    });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let resp = http.post(&form_url).form(&[("note", "hello"), ("_csrf", csrf.as_str())]).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let woke = waiter.await.unwrap();
    let messages = woke["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "{woke}");
    let data = &messages[0]["data"];
    assert_eq!(data["kind"], json!("form_submission"));
    assert_eq!(data["args"], json!({"note": "hello"}));

    let runs = extract_structured(&client.tools_call("host.runs.list", json!({})).await.unwrap());
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
    let run_id = runs["runs"][0]["run_id"].as_str().or_else(|| runs["runs"][0]["id"].as_str()).unwrap();
    assert_eq!(data["run_id"], json!(run_id));

    // Replay with the same CSRF token: refused, no second run, no second message.
    let replay = http.post(&form_url).form(&[("note", "hello"), ("_csrf", csrf.as_str())]).send().await.unwrap();
    assert_eq!(replay.status(), 403);
    let inbox = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.unwrap());
    assert_eq!(inbox["messages"].as_array().unwrap().len(), 1, "{inbox}");
    let runs = extract_structured(&client.tools_call("host.runs.list", json!({})).await.unwrap());
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
}
