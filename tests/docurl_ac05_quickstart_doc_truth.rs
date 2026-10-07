//! PRD-mcphost-docs-one-url-flow
//! AC5 (P0) -- Given the doc-truth test, When a quickstart snippet is
//! edited to call `signup`, Then the test fails naming the snippet.
//!
//! This is the doc-truth test itself (requirement 5): it parses the
//! fenced quickstart snippets from `www/llms.txt`, executes them against
//! a test server in order through `/mcp`, and asserts a published tool
//! with zero calls to `signup`/`host.redeem` and no `tenant_key`
//! argument. The second test below proves the check's own teeth: run
//! against a snippet list mutated to call `signup`, it fails and names
//! that snippet.

use crate::common;
use common::{McpClient, TestServer, python_kind_registry};
use mcphost::sandbox;
use serde_json::Value;

const LLMS_TXT: &str = include_str!("../www/llms.txt");

/// Every fenced ``` ... ``` block between "## Quickstart for agents" and
/// "## Explicit signup" in `text`, in doc order.
fn quickstart_code_blocks(text: &str) -> Vec<String> {
    let start = text
        .find("## Quickstart for agents")
        .expect("www/llms.txt must have a 'Quickstart for agents' heading");
    let end = text[start..]
        .find("## Explicit signup")
        .map(|offset| start + offset)
        .expect("'Explicit signup' heading must follow 'Quickstart for agents'");
    let section = &text[start..end];

    let mut blocks = Vec::new();
    let mut remaining = section;
    while let Some(open) = remaining.find("```") {
        let after_open = &remaining[open + 3..];
        let close = after_open
            .find("```")
            .expect("every ``` fence must close in www/llms.txt");
        blocks.push(after_open[..close].trim().to_string());
        remaining = &after_open[close + 3..];
    }
    blocks
}

/// Same doc-call-syntax parser as
/// `tests/firstpub_ac07_llms_txt_first_run_executes.rs`/
/// `tests/docurl_ac02_skill_one_url_no_ritual.rs`.
fn parse_doc_call(snippet: &str) -> (String, Value) {
    let snippet = snippet.trim();
    let open_paren = snippet.find('(').expect("doc call must have '('");
    let name = snippet[..open_paren].trim().to_string();
    let close_paren = snippet.rfind(')').expect("doc call must have ')'");
    let inner: Vec<char> = snippet[open_paren + 1..close_paren].chars().collect();

    let mut json = String::from("{");
    let mut i = 0;
    let mut depth = 0i32;
    let mut at_key_start = true;
    while i < inner.len() {
        let c = inner[i];
        if at_key_start && depth == 0 && (c.is_alphabetic() || c == '_') {
            let start = i;
            while i < inner.len() && (inner[i].is_alphanumeric() || inner[i] == '_') {
                i += 1;
            }
            let key: String = inner[start..i].iter().collect();
            json.push('"');
            json.push_str(&key);
            json.push_str("\":");
            at_key_start = false;
            while i < inner.len() && (inner[i] == ' ' || inner[i] == '=') {
                i += 1;
            }
            continue;
        }
        match c {
            '"' => {
                json.push(c);
                i += 1;
                while i < inner.len() {
                    json.push(inner[i]);
                    if inner[i] == '\\' {
                        i += 1;
                        if i < inner.len() {
                            json.push(inner[i]);
                            i += 1;
                        }
                        continue;
                    }
                    let was_quote = inner[i] == '"';
                    i += 1;
                    if was_quote {
                        break;
                    }
                }
            }
            '{' | '[' => {
                depth += 1;
                json.push(c);
                i += 1;
            }
            '}' | ']' => {
                depth -= 1;
                json.push(c);
                i += 1;
            }
            ',' if depth == 0 => {
                json.push(c);
                at_key_start = true;
                i += 1;
            }
            _ => {
                json.push(c);
                i += 1;
            }
        }
    }
    json.push('}');
    let value: Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("doc call did not parse as JSON: {json}: {e}"));
    (name, value)
}

/// The doc-truth check itself (requirement 5): runs `calls`, in order,
/// against a fresh test server through `/mcp`, on one session-bound
/// connection. `Err` names the offending call's 0-based position and tool
/// name -- either because it is `signup`/`host.redeem`, because it
/// carries a `tenant_key` argument, or because the call itself failed.
async fn run_quickstart_doc_truth(calls: &[(String, Value)]) -> Result<Value, String> {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let bare = McpClient::new(&server.base_url).with_session_continuity();

    // PRD-mcphost-session-bound-tenant-key requirement 2 (AC2/AC7): the
    // quickstart's own step 4 was updated to reconnect through
    // `onboarding.url` for every call after the first key-less one, so
    // this replay does the same -- the first snippet runs bare, every
    // later snippet runs over the tenant's own `/u/{secret}/mcp` URL that
    // first call's own onboarding envelope minted.
    let mut client = bare;
    let mut last = Value::Null;
    for (idx, (name, args)) in calls.iter().enumerate() {
        if name == "signup" || name == "host.redeem" {
            return Err(format!(
                "snippet {idx} ({name}) calls the explicit-signup ritual, not the one-URL flow"
            ));
        }
        if args.get("tenant_key").is_some() {
            return Err(format!("snippet {idx} ({name}) carries a tenant_key argument"));
        }
        last = client
            .tools_call(name, args.clone())
            .await
            .map_err(|e| format!("snippet {idx} ({name}) failed to run: {e:?}"))?;
        if idx == 0 {
            let onboarding_url = common::extract_structured(&last)["onboarding"]["url"]
                .as_str()
                .ok_or_else(|| "snippet 0's response must carry onboarding.url".to_string())?
                .to_string();
            let path = onboarding_url.trim_start_matches(&server.base_url).to_string();
            client = McpClient::new(&server.base_url).with_path(&path);
        }
    }

    let tool_list = client
        .tools_call("host.tool_list", serde_json::json!({}))
        .await
        .map_err(|e| format!("final host.tool_list failed: {e:?}"))?;
    let structured = common::extract_structured(&tool_list);
    let tools = structured["tools"].as_array().cloned().unwrap_or_default();
    if tools.is_empty() {
        return Err("the quickstart snippets ended with no published tool".to_string());
    }
    Ok(last)
}

#[tokio::test]
async fn quickstart_snippets_run_verbatim_and_end_with_a_published_tool() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let blocks = quickstart_code_blocks(LLMS_TXT);
    assert_eq!(
        blocks.len(),
        4,
        "the Quickstart section must hold exactly the whoami, publish, test, and \
         invite.create snippets: {blocks:?}"
    );
    let calls: Vec<(String, Value)> = blocks.iter().map(|b| parse_doc_call(b)).collect();

    run_quickstart_doc_truth(&calls)
        .await
        .unwrap_or_else(|e| panic!("the real quickstart snippets must run cleanly: {e}"));
}

/// AC5 itself: edit one real snippet (by replacing its tool name with
/// `signup`, the same shape a doc regression would take) and prove the
/// doc-truth check fails, naming that snippet's position and the fact
/// that it is now the ritual call.
#[tokio::test]
async fn a_snippet_edited_to_call_signup_fails_the_check_naming_the_snippet() {
    let blocks = quickstart_code_blocks(LLMS_TXT);
    let mut calls: Vec<(String, Value)> = blocks.iter().map(|b| parse_doc_call(b)).collect();

    let mutated_index = 0;
    calls[mutated_index] = ("signup".to_string(), serde_json::json!({"name": "mutated"}));

    let err = run_quickstart_doc_truth(&calls)
        .await
        .expect_err("a snippet edited to call signup must fail the doc-truth check");
    assert!(
        err.contains(&format!("snippet {mutated_index}")) && err.contains("signup"),
        "the failure must name the edited snippet: {err}"
    );
}
