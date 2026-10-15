//! PRD-mcphost-elicitation-with-ask-fallback
//! AC7 (P0) — Given a tool that elicits 4 times, When run against a fixture
//! client, Then elicits 1-3 succeed and the 4th returns
//! `elicit_calls_exceeded {limit: 3}`.

use crate::common;
use crate::elicit_session;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use elicit_session::{Reply, SessionClient};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    out = []\n    for i in range(4):\n        try:\n            out.append(mcphost.elicit(\"Rotate %d?\" % i, {\"confirm\": {\"type\": \"boolean\"}}))\n        except mcphost.elicit.ElicitError as e:\n            out.append({\"code\": e.code, \"limit\": e.data.get(\"limit\")})\n    return {\"results\": out}\n";

async fn call_ready(client: &SessionClient, name: &str, args: Value) -> Value {
    for _ in 0..200 {
        let resp = client.call(name, args.clone(), &Reply::Accept(json!({"confirm": true}))).await;
        let building = resp["result"].get("structuredContent").is_some_and(|s| s["status"] == json!("building"));
        if !building {
            return resp;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

#[tokio::test]
async fn elicits_one_to_three_succeed_and_the_fourth_is_calls_exceeded() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Melc AC7").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    let base = elicit_session::serve_session_mode(server.state.clone()).await;
    let client = SessionClient::open(&base, &key, true).await;
    let name = format!("{ns}.rotate");
    call_ready(&client, &name, json!({"warm": true})).await;

    let out = extract_structured(&call_ready(&client, &name, json!({})).await["result"]);
    let results = out["results"].as_array().expect("results");
    assert_eq!(results.len(), 4, "{out}");
    for (i, r) in results.iter().take(3).enumerate() {
        assert_eq!(r, &json!({"action": "accept", "content": {"confirm": true}}), "elicit {} must succeed: {out}", i + 1);
    }
    assert_eq!(results[3], json!({"code": "elicit_calls_exceeded", "limit": 3}), "{out}");
    assert_eq!(client.received.lock().unwrap().len(), 3, "the 4th never reaches the client");

    // The cap is per call: a fresh call gets its three again.
    let again = extract_structured(&call_ready(&client, &name, json!({})).await["result"]);
    assert_eq!(again["results"][2]["action"], json!("accept"), "{again}");
}
