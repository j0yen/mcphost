//! PRD-mcphost-first-publish-real-kind
//! AC7 (P0) -- Given `www/llms.txt`, When the "First run" section is
//! followed literally by the docs test (each call parsed and executed),
//! Then the published tool is python kind and the test call returns the
//! real output.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::Value;

const LLMS_TXT: &str = include_str!("../www/llms.txt");

/// Extracts the ordered list of fenced ``` ... ``` code blocks appearing
/// between `start_heading` and `end_heading` in `text` -- "First run"'s own
/// Publish/Test code blocks, in doc order.
fn code_blocks_between(text: &str, start_heading: &str, end_heading: &str) -> Vec<String> {
    let start = text.find(start_heading).unwrap_or_else(|| {
        panic!("www/llms.txt must have a '{start_heading}' heading");
    });
    let rest = &text[start..];
    let end = rest.find(end_heading).unwrap_or_else(|| {
        panic!("www/llms.txt must have a '{end_heading}' heading after '{start_heading}'");
    });
    let section = &rest[..end];

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

/// Converts one doc-style call snippet (`host.tool_publish(name="x",
/// kind="python", spec={"source": "..."})`) into `(rpc_name,
/// arguments_json)`. The doc's own convention already writes every value as
/// valid JSON (quoted strings with JSON escaping, `{...}`/`[...]` for
/// objects/arrays) -- the only non-JSON part is `key=value` instead of
/// `"key": value` -- so this just re-punctuates each top-level `key=` into
/// `"key":` and wraps the result in `{}`, tracking string/brace depth so a
/// `=`, `,`, `{` or `}` inside a string or nested value is never mistaken
/// for a top-level separator.
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

#[tokio::test]
async fn first_run_publish_then_test_is_a_real_python_tool() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let blocks = code_blocks_between(LLMS_TXT, "### Publish", "### Schedule");
    assert_eq!(
        blocks.len(),
        2,
        "First run's Publish..Schedule span must hold exactly the publish and test code blocks: {blocks:?}"
    );
    let (publish_call, publish_args) = parse_doc_call(&blocks[0]);
    let (test_call, test_args) = parse_doc_call(&blocks[1]);
    assert_eq!(publish_call, "host.tool_publish");
    assert_eq!(test_call, "host.tool_test");
    assert_eq!(
        publish_args["kind"], "python",
        "First run's documented publish must be python kind: {publish_args}"
    );

    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let publish_result = client
        .tools_call(&publish_call, publish_args)
        .await
        .expect("First run's own publish call must succeed verbatim");
    let published_kind = extract_structured(&publish_result)["kind"].clone();
    assert_eq!(published_kind, Value::String("python".to_string()));

    let test_result = client
        .tools_call(&test_call, test_args)
        .await
        .expect("First run's own test call must succeed verbatim");
    let structured = extract_structured(&test_result);
    assert_eq!(
        structured["result"]["reversed"],
        Value::String("olleh".to_string()),
        "First run's test call must return the real output: {structured}"
    );
    assert_eq!(structured["result"]["words"], Value::from(1));
}
