//! PRD-mcphost-capabilities-generated-from-dispatch
//! AC2 (P0) -- Given every method of every `CAPABILITIES` row, When each is
//! sent four ways (valid params / `{}`, with / without `Mcp-Method`) to `/mcp`
//! and to `/t/{ns}/mcp`, Then every `implemented: false` row method and every
//! `server_to_client` method answers `-32601` with `data.capability ==
//! row.name` and `data.help_url` ending `/help/<data.code>`, and every
//! implemented client-to-server method answers a `result` for the valid body.
//! One divergent cell fails the test naming method, route and variant.

use crate::common;
use common::TestServer;
use mcphost::capabilities::CAPABILITIES;
use serde_json::{Value, json};

/// Minimal valid params per method; a method not listed takes `{}`.
fn params_for(method: &str, n: usize) -> Value {
    match method {
        "tools/call" => json!({"name": "signup", "arguments": {"name": format!("cgd ac02 {n}")}}),
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => json!({"uri": "mcphost://x"}),
        "prompts/get" => json!({"name": "x"}),
        "completion/complete" => {
            let body: Value = serde_json::from_str(include_str!("fixtures/clive_complete_keyless.json")).expect("fixture is JSON");
            body["params"].clone()
        }
        "logging/setLevel" => json!({"level": "info"}),
        "sampling/createMessage" => json!({"messages": [], "maxTokens": 1}),
        "elicitation/create" => json!({"message": "x", "requestedSchema": {"type": "object", "properties": {}}}),
        _ => json!({}),
    }
}

#[tokio::test]
async fn cgd_ac02_every_row_method_answers_as_its_row_says_on_every_route_and_variant() {
    let server = TestServer::start().await;
    common::bare_tenant(&server.state, "cgdtwo").await;
    let routes = ["/mcp", "/t/cgdtwo/mcp"];
    let mut failures = Vec::new();
    let mut cells = 0usize;
    for row in CAPABILITIES {
        for method in row.methods {
            let refused = !row.implemented || row.server_to_client.contains(method);
            for route in routes {
                for (valid, with_header) in [(true, true), (true, false), (false, true), (false, false)] {
                    cells += 1;
                    let variant = format!(
                        "{} params, {} Mcp-Method",
                        if valid { "valid" } else { "{}" },
                        if with_header { "with" } else { "without" }
                    );
                    let params = if valid { params_for(method, cells) } else { json!({}) };
                    let reply = common::raw_rpc(&server.base_url, route, method, params, with_header).await;
                    let cell = format!("{method} @ {route} [{variant}]");
                    if refused {
                        let err = &reply["error"];
                        let code = err["data"]["code"].as_str().unwrap_or_default();
                        let help = err["data"]["help_url"].as_str().unwrap_or_default();
                        if err["code"] != -32601 {
                            failures.push(format!("{cell}: want -32601, got {reply}"));
                        } else if err["data"]["capability"] != row.name {
                            failures.push(format!("{cell}: data.capability must be `{}`, got {reply}", row.name));
                        } else if code.is_empty() || !help.ends_with(&format!("/help/{code}")) {
                            failures.push(format!("{cell}: data.help_url must end /help/{code:?}, got {reply}"));
                        }
                    } else if valid && reply.get("result").is_none() {
                        failures.push(format!("{cell}: implemented method must answer a result, got {reply}"));
                    }
                }
            }
        }
    }
    assert!(cells >= 8 * 8, "only {cells} cells exercised");
    assert!(failures.is_empty(), "drift between CAPABILITIES and the wire:\n{}", failures.join("\n"));
}
