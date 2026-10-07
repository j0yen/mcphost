//! PRD-mcphost-tools-list-alias-truth
//! AC5 — Given the contract test, When run, Then tools/list ⊇ alias table ∪
//! flattened forms and every `next:` hint in quickstart, publish, and test
//! responses names an advertised tool.

use crate::common;
use common::{McpClient, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::tool_aliases::{TOOL_ALIASES, flattened_form};
use serde_json::{Value, json};
use std::collections::HashSet;

/// Every tool name a `next`/`call` slot in `v` points at: `next` as a bare
/// string, `{tool}`, or a list of steps whose `call`/`tool` is the name.
fn next_hint_names(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                match (k.as_str(), child) {
                    ("next", Value::String(s)) => out.push(s.clone()),
                    ("next", Value::Object(o)) => {
                        if let Some(t) = o.get("tool").and_then(Value::as_str) {
                            out.push(t.to_string());
                        }
                    }
                    ("call", Value::String(s)) => out.push(s.clone()),
                    _ => {}
                }
                next_hint_names(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| next_hint_names(i, out)),
        _ => {}
    }
}

#[tokio::test]
async fn tools_list_is_a_superset_and_every_next_hint_is_advertised() {
    // The python kind is registered (not run) so the default quickstart,
    // whose starter recipe is python, answers.
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AliasTruth AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    // No bearer: the tenant_key argument path is the one that attaches
    // `next` hints to ordinary host.* responses.
    let keyed = McpClient::new(&server.base_url);

    let listed = client.tools_list().await.expect("tools/list");
    let advertised: HashSet<String> =
        listed["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();

    // tools/list ⊇ alias table ∪ flattened forms.
    for a in TOOL_ALIASES {
        assert!(advertised.contains(a.alias), "alias {} not advertised", a.alias);
        let flat = flattened_form(a.canonical).unwrap();
        assert!(advertised.contains(&flat), "flattened {flat} not advertised");
    }
    for name in advertised.iter().filter(|n| n.starts_with("host.")) {
        let flat = flattened_form(name).unwrap();
        assert!(advertised.contains(&flat), "flattened {flat} of {name} not advertised");
    }

    // Live responses: quickstart (default, echo, python), publish, test.
    let mut responses: Vec<Value> = Vec::new();
    for args in [json!({}), json!({"kind": "echo"}), json!({"kind": "python"})] {
        responses.push(extract_structured(&client.tools_call("host.quickstart", args).await.expect("quickstart")));
    }
    let published = keyed
        .tools_call(
            "host.tool.publish",
            json!({"tenant_key": key, "name": "hinted", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    responses.push(extract_structured(&published));
    let tested = keyed
        .tools_call("host.tool.test", json!({"tenant_key": key, "name": "hinted", "args": {}}))
        .await
        .expect("test");
    responses.push(extract_structured(&tested));

    let mut hinted = Vec::new();
    for r in &responses {
        next_hint_names(r, &mut hinted);
    }
    // The static hint table is where every other `next` comes from.
    for (_, tool, _) in mcphost::control::next_hint_table() {
        hinted.push((*tool).to_string());
    }
    assert!(hinted.len() > 5, "expected hints in the responses, got {hinted:?}");
    for name in &hinted {
        // A step that calls the tenant's own not-yet-published example tool
        // (`<ns>.my_tool`) is a placeholder, not a control-plane name.
        if name.starts_with(&format!("{ns}.")) {
            continue;
        }
        assert!(advertised.contains(name), "`next` hint names {name}, which tools/list does not advertise");
    }
}
