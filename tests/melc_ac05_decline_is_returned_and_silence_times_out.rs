//! PRD-mcphost-elicitation-with-ask-fallback
//! AC5 (P0) — Given a fixture client that answers `decline`, When the tool
//! elicits, Then `{action: "decline"}` is returned unchanged; Given a client
//! that never answers, Then `elicit_timeout` at `timeout_s` and the tool still
//! returns inside its budget.

use crate::common;
use crate::elicit_session;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use elicit_session::{Reply, SessionClient};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    try:\n        return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}}, timeout_s=args.get(\"timeout_s\"))\n    except mcphost.elicit.ElicitError as e:\n        return {\"code\": e.code, \"timeout_s\": e.data.get(\"timeout_s\")}\n";

async fn call_ready(client: &SessionClient, name: &str, args: Value, reply: &Reply) -> Value {
    for _ in 0..200 {
        let resp = client.call(name, args.clone(), reply).await;
        let building = resp["result"].get("structuredContent").is_some_and(|s| s["status"] == json!("building"));
        if !building {
            return resp;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

#[tokio::test]
async fn decline_comes_back_unchanged_and_a_silent_client_times_out_inside_the_budget() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Melc AC5").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    let base = elicit_session::serve_session_mode(server.state.clone()).await;
    let client = SessionClient::open(&base, &key, true).await;
    let name = format!("{ns}.rotate");
    call_ready(&client, &name, json!({"warm": true}), &Reply::Decline).await;

    let declined = extract_structured(&client.call(&name, json!({}), &Reply::Decline).await["result"]);
    assert_eq!(declined, json!({"action": "decline"}), "decline must come back unchanged");
    let rows = extract_structured(&admin.tools_call("host.ask.list", json!({})).await.expect("list"));
    assert_eq!(rows["asks"][0]["channel"], json!("client"), "{rows}");
    assert_eq!(rows["asks"][0]["choice"], json!("declined"), "{rows}");

    let silent = crate::deadline_budget!(15_000, at_least = 900, {
        extract_structured(&client.call(&name, json!({"timeout_s": 1}), &Reply::Silent).await["result"])
    });
    assert_eq!(silent["code"], json!("elicit_timeout"), "{silent}");
    assert_eq!(silent["timeout_s"], json!(1), "{silent}");
}
