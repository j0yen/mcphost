//! PRD-mcphost-sampling-from-tools
//! AC4 (P0) — Given a tool that asks 5 times in one call, When it runs
//! against a supporting client, Then asks 1-4 succeed and the 5th returns
//! `sampling_calls_exceeded {limit: 4}`.

use crate::common;
use crate::elicit_session;
use crate::sampling_session;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use sampling_session::{Answer, SamplingSession};
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    out = []\n    for i in range(5):\n        try:\n            out.append(mcphost.sampling.ask(\"Urgent %d?\" % i, max_tokens=4)[\"text\"])\n        except mcphost.sampling.SamplingError as e:\n            out.append({\"code\": e.code, \"limit\": e.data.get(\"limit\")})\n    return {\"results\": out}\n";

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
async fn asks_one_to_four_succeed_and_the_fifth_is_calls_exceeded() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Msmp AC4").await;
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
    let answer = Answer::text("yes").with_usage(0, 1);
    let name = format!("{ns}.triage");
    call_ready(&client, &name, json!({"warm": true}), &answer).await;

    let out = extract_structured(&call_ready(&client, &name, json!({}), &answer).await["result"]);
    let results = out["results"].as_array().expect("results");
    assert_eq!(results.len(), 5, "{out}");
    for (i, r) in results.iter().take(4).enumerate() {
        assert_eq!(r, &json!("yes"), "ask {} must succeed: {out}", i + 1);
    }
    assert_eq!(results[4], json!({"code": "sampling_calls_exceeded", "limit": 4}), "{out}");
    assert_eq!(client.received.lock().unwrap().len(), 4, "the 5th never reaches the client");

    // The cap is per call: a fresh call gets its four again.
    let again = extract_structured(&call_ready(&client, &name, json!({}), &answer).await["result"]);
    assert_eq!(again["results"][3], json!("yes"), "{again}");
}
