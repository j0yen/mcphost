//! PRD-mcphost-ask-user-decision
//! AC1 -- Given a 0.106.0-shaped test host, When a test composes the
//! blocking question from `host.tool_share(visibility="url")` on an `answer`
//! tool plus `host.msg.wait`, and then runs `host.ask.create` +
//! `host.ask.wait`, Then it records both call counts and end-to-end
//! latencies in the test output, and the verb path uses strictly fewer host
//! calls (the PRD is parked if not).

use crate::common;

use common::{McpClient, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::{Duration, Instant};

/// The composed path's own `answer` tool: writes the visitor's choice into
/// the agent's inbox, the only place `host.msg.wait` can read from.
const ANSWER_SOURCE: &str =
    "from mcphost import msg\ndef main(args):\n    return msg.send(args[\"to\"], {\"choice\": args[\"choice\"]})\n";

#[tokio::test]
async fn verb_path_uses_strictly_fewer_host_calls_than_the_composed_path() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "askd ac01 tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // ---- composed: publish `answer`, url-share it, wait on the inbox ----
    let composed_started = Instant::now();
    let mut composed_calls = 0u32;
    client
        .tools_call("host.tool_publish", json!({"name": "answer", "kind": "python", "spec": {"source": ANSWER_SOURCE}}))
        .await
        .expect("publish answer");
    composed_calls += 1;
    let shared = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "answer", "visibility": "url"}))
            .await
            .expect("tool_share"),
    );
    composed_calls += 1;
    let url = shared["url"].as_str().expect("share url").to_string();

    // The python environment builds on first use; warm it with the direct
    // call so the visitor's POST below measures delivery, not env build.
    let _ = poll_until_ready(&client, &format!("{ns}.answer"), json!({"to": ns, "choice": "warmup"}), Duration::from_secs(30)).await;
    let drained = extract_structured(&client.tools_call("host.msg.inbox", json!({})).await.expect("inbox"));
    let cursor = drained["next_cursor"].clone();

    let path = url.split_once("/x/").map(|(_, p)| p).expect("/x/ url");
    let visitor = reqwest::Client::new()
        .post(format!("{}/x/{path}", server.base_url))
        .json(&json!({"to": ns, "choice": "yes"}))
        .send()
        .await
        .expect("visitor post");
    assert!(visitor.status().is_success(), "visitor post: {}", visitor.status());
    let mut wait_args = json!({"timeout_s": 5, "unread_only": true});
    if cursor.is_string() {
        wait_args["cursor"] = cursor;
    }
    let waited = extract_structured(&client.tools_call("host.msg.wait", wait_args).await.expect("msg.wait"));
    composed_calls += 1;
    assert!(
        waited["messages"].as_array().is_some_and(|m| m.iter().any(|x| x["data"]["choice"] == "yes")),
        "composed path must deliver the choice: {waited}"
    );
    let composed_ms = composed_started.elapsed().as_millis();

    // ---- verbs: create + wait ----
    let verb_started = Instant::now();
    let mut verb_calls = 0u32;
    let created = extract_structured(
        &client
            .tools_call("host.ask.create", json!({"question": "Deploy?", "options": ["yes", "no"]}))
            .await
            .expect("ask.create"),
    );
    verb_calls += 1;
    let ask_id = created["ask_id"].as_str().expect("ask_id").to_string();
    assert!(created["url"].as_str().is_some_and(|u| u.contains("/a/")), "{created}");
    let pending = extract_structured(
        &client.tools_call("host.ask.wait", json!({"ask_id": ask_id, "timeout_s": 1})).await.expect("ask.wait"),
    );
    verb_calls += 1;
    assert_eq!(pending["status"], json!("pending"), "{pending}");
    let verb_ms = verb_started.elapsed().as_millis();

    println!("askd_ac01 composed: host_calls={composed_calls} end_to_end_ms={composed_ms}");
    println!("askd_ac01 verbs:    host_calls={verb_calls} end_to_end_ms={verb_ms}");
    assert!(
        verb_calls < composed_calls,
        "the verb path must use strictly fewer host calls (verbs {verb_calls} vs composed {composed_calls}); otherwise the PRD is parked"
    );
}
