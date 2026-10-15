//! PRD-mcphost-elicitation-with-ask-fallback
//! AC3 (P0) — Given a schema with a nested object or 9 properties, When
//! `mcphost.elicit` is called, Then `elicit_schema_invalid {path, allowed}` is
//! raised and no client request and no ask row exist.

use crate::common;
use crate::elicit_session;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use elicit_session::{Reply, SessionClient};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

const TOOL: &str = "import mcphost\ndef main(args):\n    kind = args.get(\"kind\")\n    if kind == \"nested\":\n        schema = {\"address\": {\"type\": \"object\", \"properties\": {\"street\": {\"type\": \"string\"}}}}\n    else:\n        schema = {\"p%d\" % i: {\"type\": \"string\"} for i in range(9)}\n    try:\n        return mcphost.elicit(\"Where?\", schema)\n    except mcphost.elicit.ElicitError as e:\n        return {\"code\": e.code, \"path\": e.data.get(\"path\"), \"allowed\": e.data.get(\"allowed\")}\n";

async fn call_ready(client: &SessionClient, name: &str, args: Value) -> Value {
    for _ in 0..200 {
        let resp = client.call(name, args.clone(), &Reply::Accept(json!({}))).await;
        let building = resp["result"].get("structuredContent").is_some_and(|s| s["status"] == json!("building"));
        if !building {
            return resp;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tool never became ready");
}

#[tokio::test]
async fn nested_object_and_nine_properties_raise_elicit_schema_invalid_with_no_request_and_no_row() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Melc AC3").await;
    let admin = McpClient::with_bearer(&server.base_url, &key);
    admin
        .tools_call("host.tool_publish", json!({"name": "where", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    let base = elicit_session::serve_session_mode(server.state.clone()).await;
    let client = SessionClient::open(&base, &key, true).await;
    let name = format!("{ns}.where");

    let nested = extract_structured(&call_ready(&client, &name, json!({"kind": "nested"})).await["result"]);
    assert_eq!(nested["code"], json!("elicit_schema_invalid"), "{nested}");
    assert_eq!(nested["path"], json!("$.address"), "{nested}");
    assert_eq!(nested["allowed"], json!(mcphost::elicit::ELICIT_SCHEMA_SUBSET.types), "{nested}");

    let nine = extract_structured(&call_ready(&client, &name, json!({"kind": "nine"})).await["result"]);
    assert_eq!(nine["code"], json!("elicit_schema_invalid"), "{nine}");
    assert_eq!(nine["path"], json!("$"), "{nine}");
    assert_eq!(nine["allowed"], json!(mcphost::elicit::ELICIT_SCHEMA_SUBSET.types), "{nine}");

    assert!(client.received.lock().unwrap().is_empty(), "no elicitation/create may reach the client");
    let rows = extract_structured(&admin.tools_call("host.ask.list", json!({})).await.expect("list"));
    assert_eq!(rows["asks"], json!([]), "no ask row may exist: {rows}");
}
