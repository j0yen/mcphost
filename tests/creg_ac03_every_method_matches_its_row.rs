//! AC3 (P0) -- PRD-mcphost-server-capabilities-from-registry. Every method of
//! every `CAPABILITIES` row is sent to an in-process server: implemented rows
//! answer a `result`, unimplemented rows `-32601` with
//! `data.help_url` ending `/help/capability_unsupported` and `data.alternative`
//! equal to the row's. A row with no `methods` fails the test naming it.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::capabilities::{CAPABILITIES, Capability};
use serde_json::{Value, json};

/// Minimal valid params per method; a method not listed takes `{}`.
fn params_for(method: &str) -> Value {
    match method {
        "tools/call" => json!({"name": "signup", "arguments": {"name": "creg ac03"}}),
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => json!({"uri": "mcphost://x"}),
        "prompts/get" => json!({"name": "x"}),
        "completion/complete" => json!({
            "ref": {"type": "ref/prompt", "name": "x"},
            "argument": {"name": "a", "value": "v"}
        }),
        "logging/setLevel" => json!({"level": "info"}),
        "sampling/createMessage" => json!({"messages": [], "maxTokens": 1}),
        "elicitation/create" => json!({
            "message": "x",
            "requestedSchema": {"type": "object", "properties": {}}
        }),
        _ => json!({}),
    }
}

/// The row-level precondition the drift test needs: a row names its methods.
fn methods_problem(row: &Capability) -> Option<String> {
    row.methods.is_empty().then(|| format!("capability row `{}` has an empty `methods` list", row.name))
}

#[tokio::test]
async fn every_method_of_every_row_answers_as_its_row_says() {
    let problems: Vec<String> = CAPABILITIES.iter().filter_map(methods_problem).collect();
    assert!(problems.is_empty(), "{}", problems.join("; "));

    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let mut failures = Vec::new();
    for row in CAPABILITIES {
        for method in row.methods {
            let reply = client.rpc(method, params_for(method)).await;
            if row.implemented {
                if reply.get("result").is_none() {
                    failures.push(format!("{}: implemented row `{}` must answer a result, got {reply}", method, row.name));
                }
                continue;
            }
            let err = &reply["error"];
            let help_url = err["data"]["help_url"].as_str().unwrap_or_default();
            if err["code"] != -32601 {
                failures.push(format!("{method}: row `{}` must answer -32601, got {reply}", row.name));
            } else if !help_url.ends_with("/help/capability_unsupported") {
                failures.push(format!("{method}: data.help_url must end /help/capability_unsupported, got {reply}"));
            } else if err["data"]["alternative"] != json!(row.alternative) {
                failures.push(format!("{method}: data.alternative must be {:?}, got {reply}", row.alternative));
            } else if err["data"]["capability"] != row.name {
                failures.push(format!("{method}: data.capability must be `{}`, got {reply}", row.name));
            }
        }
    }
    assert!(failures.is_empty(), "drift between CAPABILITIES and the wire:\n{}", failures.join("\n"));
}

#[test]
fn a_row_with_empty_methods_fails_naming_the_row() {
    let row = Capability {
        name: "widgets",
        methods: &[],
        implemented: false,
        list_changed: false,
        subscribe: false,
        ceiling: None,
        probe: "widgets",
        alternative: None,
        help_code: "capability_unsupported",
    };
    let problem = methods_problem(&row).expect("an empty methods list is a problem");
    assert!(problem.contains("widgets"), "{problem}");
}
