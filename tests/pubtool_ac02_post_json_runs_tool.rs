//! PRD-mcphost-public-tool-url
//! AC2 (P0) — Given that URL, When
//! `curl -s -X POST <url> -H 'content-type: application/json' -d '{"text":"hi"}'`
//! runs, Then the response is 200 with `ok = true`, `result.text = "hi"`,
//! and a `run_id` that `host.runs.get` resolves with `caller = "url"`.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn post_json_body_runs_the_tool_and_run_resolves_with_caller_url() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "echo",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
            }),
        )
        .await
        .expect("publish ok");
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("tool_share ok"),
    );
    let url = shared["url"].as_str().expect("url field").to_string();

    let http = reqwest::Client::new();
    let resp = http
        .post(&url)
        .header("content-type", "application/json")
        .body(r#"{"text":"hi"}"#)
        .send()
        .await
        .expect("POST /x/<token>/echo");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["result"]["text"], json!("hi"));
    let run_id = body["run_id"].as_str().expect("run_id in response").to_string();

    let run = extract_structured(
        &client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("runs.get"),
    );
    assert_eq!(run["trigger"], json!("url"), "run must resolve with caller (trigger) = url: {run:?}");
    assert_eq!(run["status"], json!("done"));
}
