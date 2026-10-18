//! PRD-mcphost-sampling-from-tools
//! AC6 (P0) — Given a successful ask, When `host.tool.logs` is read, Then the
//! entry holds `sampling: {asks: 1, tokens: 212}` and no substring of the
//! prompt or the answer.
//!
//! The client reports `_meta.usage` of 200 input + 12 output tokens (212).

use crate::common;
use crate::elicit_session;
use crate::sampling_session;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use sampling_session::{Answer, SamplingSession};
use serde_json::{Value, json};
use std::time::Duration;

const PROMPT: &str = "classified-prompt-marker: is the ticket urgent?";
const REPLY: &str = "secret-answer-marker";

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    r = mcphost.sampling.ask(\"classified-prompt-marker: is the ticket urgent?\", max_tokens=8)\n    return {\"len\": len(r[\"text\"]), \"tokens\": r[\"tokens\"]}\n";

async fn call_ready(client: &SamplingSession, name: &str, args: Value, answer: &Answer) -> Value {
    for _ in 0..200 {
        let resp = client.call(name, args.clone(), answer).await;
        let building = resp["result"].get("structuredContent").is_some_and(|s| s["status"] == json!("building"));
        if !building {
            return resp;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

#[tokio::test]
async fn the_log_entry_has_asks_and_tokens_and_no_prompt_or_answer() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Msmp AC6").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call(
            "host.tool_publish",
            json!({"name": "triage", "kind": "python", "spec": {"source": TOOL, "timeout_s": 40}}),
        )
        .await
        .expect("publish");
    let base = elicit_session::serve_session_mode(server.state.clone()).await;
    let client = SamplingSession::open(&base, &key, true).await;
    let answer = Answer::text(REPLY).with_usage(200, 12);
    let name = format!("{ns}.triage");
    call_ready(&client, &name, json!({"warm": true}), &answer).await;

    let out = extract_structured(&client.call(&name, json!({}), &answer).await["result"]);
    assert_eq!(out["tokens"], json!(212), "{out}");

    let mut lines: Vec<String> = Vec::new();
    for _ in 0..50 {
        let logs = extract_structured(&admin.tools_call("host.tool.logs", json!({"name": "triage"})).await.expect("logs"));
        lines = logs["lines"].as_array().expect("lines").iter().filter_map(|l| l.as_str().map(str::to_string)).collect();
        if lines.iter().any(|l| l.contains("\"sampling\"")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let entries: Vec<Value> = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v.get("sampling").is_some())
        .collect();
    assert_eq!(entries.len(), 1, "one sampling entry for one ask: {lines:?}");
    assert_eq!(entries[0]["sampling"]["asks"], json!(1), "{lines:?}");
    assert_eq!(entries[0]["sampling"]["tokens"], json!(212), "{lines:?}");

    let everything = lines.join("\n");
    for secret in [PROMPT, "classified-prompt-marker", "urgent?", REPLY, "secret-answer-marker"] {
        assert!(!everything.contains(secret), "{secret:?} must not be in host.tool.logs: {lines:?}");
    }
}
