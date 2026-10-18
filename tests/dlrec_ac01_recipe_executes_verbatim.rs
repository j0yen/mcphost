//! PRD-mcphost-delegation-recipes-and-quickstart
//! AC1 (P0) — Given a test server and a fresh tenant, When
//! `sub_agent_key().calls` are sent in order verbatim, Then call 1 returns
//! `key` and `label`, call 2 returns exactly the code the recipe names, call 3
//! returns `revoked >= 1`, and every scope and code in the rendered text
//! appears in the calls' requests or responses.

use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::delegation_recipe::{Caller, sub_agent_key};
use serde_json::{Value, json};

#[tokio::test]
async fn dlrec_ac01_recipe_executes_verbatim() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Recipe Tenant").await;
    let lead = McpClient::with_bearer(&server.base_url, &key);
    let recipe = sub_agent_key();
    assert_eq!(recipe.calls.len(), 3);

    let mut helper: Option<McpClient> = None;
    let mut transcript = String::new();
    for (i, call) in recipe.calls.iter().enumerate() {
        let client = match call.caller {
            Caller::Lead => &lead,
            Caller::Helper => helper.as_ref().expect("call 1 minted the helper key"),
        };
        transcript.push_str(&call.arguments.to_string());
        let outcome = client.tools_call(&call.tool, call.arguments.clone()).await;
        match (&call.expect.error_code, outcome) {
            (None, Ok(raw)) => {
                let out: Value = extract_structured(&raw);
                transcript.push_str(&out.to_string());
                for f in &call.expect.ok_fields {
                    assert!(out.get(f).is_some(), "call {}: missing `{f}` in {out}", i + 1);
                }
                if let Some(a) = &call.expect.ok_at_least {
                    assert!(out[&a.field].as_i64().unwrap_or(-1) >= a.min, "call {}: {out}", i + 1);
                }
                if i == 0 {
                    helper = Some(McpClient::with_bearer(&server.base_url, out["key"].as_str().expect("key")));
                }
            }
            (Some(code), Err(err)) => {
                assert_eq!(err.error_code.as_deref(), Some(code.as_str()), "call {}: {err:?}", i + 1);
                transcript.push_str(&format!("{} {}", code, err.data));
            }
            (want, got) => panic!("call {}: wanted {want:?}, got {got:?}", i + 1),
        }
        if let Some(code) = &call.expect.helper_next_call_error {
            let err = helper
                .as_ref()
                .expect("helper")
                .tools_call("host.whoami", json!({}))
                .await
                .expect_err("revoked helper must fail");
            assert_eq!(err.error_code.as_deref(), Some(code.as_str()), "{err:?}");
            transcript.push_str(&format!("{} {}", code, err.data));
        }
    }

    let rendered = mcphost::llms_txt::render_recipe(&recipe);
    for code in [mcphost::scopes::SCOPE_ESCALATION, mcphost::scopes::KEY_REVOKED] {
        assert!(rendered.contains(code), "rendered text names {code}");
        assert!(transcript.contains(code), "{code} appears in the executed calls");
    }
    // Derived from the registry, not counted: every scope `tools:call` does not
    // grant is asked for by the executed calls.
    for row in mcphost::scopes::catalog() {
        if !row.granted_by(mcphost::scopes::Scope::ToolsCall.as_str()) && row.name != mcphost::scopes::Scope::Mcp.as_str() {
            assert!(transcript.contains(&format!("\"{}\"", row.name)), "scope {} is never asked for by the executed calls", row.name);
        }
    }
    for row in mcphost::scopes::catalog() {
        if rendered.contains(&format!("`{}`", row.name)) {
            assert!(transcript.contains(&row.name), "scope {} is in the text but never in the calls", row.name);
        }
    }
}
