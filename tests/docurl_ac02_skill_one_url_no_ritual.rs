//! PRD-mcphost-docs-one-url-flow
//! AC2 (P0) -- Given `/skill.md` and the plugin skill, When an agent
//! follows the steps against a test server through `/mcp`, Then it has a
//! tenant and a published tool without calling `signup` or
//! `host.redeem`.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::Value;

const SKILL_MD: &str = include_str!("../plugin/skills/mcphost/SKILL.md");

/// www/skill.md and the plugin skill must be byte-identical (requirement
/// 2 / `tests/www_pages.sh`'s own `cmp` check) -- asserted again here so
/// this file's own parsing of `SKILL_MD` also speaks for `www/skill.md`.
#[test]
fn www_skill_md_is_byte_identical_to_the_plugin_skill() {
    let www_skill_md = include_str!("../www/skill.md");
    assert_eq!(
        www_skill_md, SKILL_MD,
        "www/skill.md must be byte-identical to plugin/skills/mcphost/SKILL.md"
    );
}

/// Every fenced ``` ... ``` block appearing before `end_heading` in
/// `text` -- the skill's own one-URL steps, in doc order, stopping before
/// the "Explicit signup" fallback section so that ritual's own `signup`/
/// `host.redeem` snippets are never collected.
fn code_blocks_before(text: &str, end_heading: &str) -> Vec<String> {
    let end = text
        .find(end_heading)
        .unwrap_or_else(|| panic!("SKILL.md must have a '{end_heading}' heading"));
    let section = &text[..end];

    let mut blocks = Vec::new();
    let mut remaining = section;
    while let Some(open) = remaining.find("```") {
        let after_open = &remaining[open + 3..];
        let close = after_open
            .find("```")
            .expect("every ``` fence must close in SKILL.md");
        blocks.push(after_open[..close].trim().to_string());
        remaining = &after_open[close + 3..];
    }
    blocks
}

/// Converts one doc-style call snippet (`host.tool_publish(name="x",
/// kind="echo", spec={"schema": {"type": "object"}})`) into `(rpc_name,
/// arguments_json)` -- same convention as
/// `tests/firstpub_ac07_llms_txt_first_run_executes.rs`'s own parser.
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
async fn skill_md_steps_create_a_tenant_and_publish_a_tool_with_no_ritual_call() {
    let blocks = code_blocks_before(SKILL_MD, "## Explicit signup");
    assert_eq!(
        blocks.len(),
        3,
        "the skill's one-URL steps must hold exactly the whoami, publish, and \
         trigger-set snippets: {blocks:?}"
    );

    let calls: Vec<(String, Value)> = blocks.iter().map(|b| parse_doc_call(b)).collect();
    for (name, _) in &calls {
        assert_ne!(name, "signup", "the skill's one-URL steps must never call signup");
        assert_ne!(name, "host.redeem", "the skill's one-URL steps must never call host.redeem");
        assert!(
            !name.contains("tenant_key"),
            "the skill's one-URL steps must never reference a tenant_key argument by name"
        );
    }

    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url).with_session_continuity();

    for (name, args) in &calls {
        assert!(
            args.get("tenant_key").is_none(),
            "step {name} must not carry a tenant_key argument"
        );
        client
            .tools_call(name, args.clone())
            .await
            .unwrap_or_else(|e| panic!("skill step {name}({args}) must succeed verbatim: {e:?}"));
    }

    // Given the implicit tenant this session now has, the published
    // `hello` tool is really there.
    let tool_list = client
        .tools_call("host.tool_list", serde_json::json!({}))
        .await
        .expect("host.tool_list on the now-bound session");
    let structured = common::extract_structured(&tool_list);
    let tools = structured["tools"].as_array().expect("tools array");
    assert!(
        tools.iter().any(|t| t["name"]
            .as_str()
            .map(|n| n == "hello" || n.ends_with(".hello"))
            .unwrap_or(false)),
        "the skill's steps must have published a tool named 'hello': {tools:?}"
    );

    let whoami = client
        .tools_call("host.whoami", serde_json::json!({}))
        .await
        .expect("host.whoami on the now-bound session");
    let whoami = common::extract_structured(&whoami);
    assert!(
        whoami["namespace"].as_str().is_some(),
        "the session must now be a real tenant: {whoami}"
    );
}
