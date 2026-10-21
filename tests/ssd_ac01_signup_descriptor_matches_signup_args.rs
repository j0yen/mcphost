//! AC1 (hotfix signup-source-descriptor) -- `signup`'s served descriptor, the
//! argument names `control::signup` reads, and the generated SKILL.md /
//! www/skill.md argument block all derive from `control::SIGNUP_ARGS`; the
//! `source` pattern is the one `state::SOURCE_PATTERN` that
//! `state::is_valid_signup_source` enforces.
//!
//! Found by skill-walk 19/21 (chain run 211800Z): `signup: unknown argument
//! "source"; schema has "handoff", "name"` at SKILL.md:92 and www/skill.md:92
//! -- the handler accepted `source` since PRD signup-kill-switch-and-source but
//! the descriptor never declared it.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::control::{
    SIGNUP_ARGS, SIGNUP_ARGS_DOCS_END, SIGNUP_ARGS_DOCS_START, render_signup_args_block,
};
use mcphost::state::{SOURCE_PATTERN, is_valid_signup_source};
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;

const CONTROL_RS: &str = include_str!("../src/control.rs");
const SKILL_MD: &str = include_str!("../plugin/skills/mcphost/SKILL.md");
const WWW_SKILL_MD: &str = include_str!("../www/skill.md");

#[tokio::test]
async fn served_signup_descriptor_properties_equal_signup_args() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let tools = client.tools_list().await.expect("anonymous tools/list");
    let signup = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == "signup")
        .expect("anonymous tools/list lists signup");
    let schema = &signup["inputSchema"];

    let served: BTreeSet<String> = schema["properties"]
        .as_object()
        .expect("signup inputSchema.properties")
        .keys()
        .cloned()
        .collect();
    let declared: BTreeSet<String> = SIGNUP_ARGS.iter().map(|a| a.name.to_string()).collect();
    assert_eq!(served, declared, "signup's served properties must equal SIGNUP_ARGS");

    let required: BTreeSet<String> = schema["required"]
        .as_array()
        .expect("signup inputSchema.required")
        .iter()
        .map(|v| v.as_str().expect("required entry is a string").to_string())
        .collect();
    let want_required: BTreeSet<String> =
        SIGNUP_ARGS.iter().filter(|a| a.required).map(|a| a.name.to_string()).collect();
    assert_eq!(required, want_required);
    assert!(declared.contains("source"), "source must be a declared signup argument");
    assert_eq!(schema["additionalProperties"], Value::Bool(false));

    let source = &schema["properties"]["source"];
    assert_eq!(source["type"], "string");
    assert_eq!(source["pattern"], SOURCE_PATTERN, "the served pattern is the one constant");
    let description = source["description"].as_str().expect("source description");
    for needle in ["signup_source", "synthetic", "synthorg"] {
        assert!(description.contains(needle), "source description must mention {needle}: {description}");
    }
}

#[test]
fn source_pattern_agrees_with_the_validator_over_a_corpus() {
    let re = Regex::new(SOURCE_PATTERN).expect("SOURCE_PATTERN compiles as a Rust regex");
    let mut corpus: Vec<String> = vec![
        "", "a", "0", "plugin", "www", "cli", "hn", "synthorg", "docs.quickstart", "a.b_c-d",
        "A", "Plugin", "-a", ".a", "_a", "a b", "a:b", "a!", "HN!", "é", "aé", "a\n", "\na", "a\0",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for n in [1, 2, 63, 64, 65, 66, 200] {
        corpus.push("a".repeat(n));
        corpus.push(format!("1{}", "-".repeat(n - 1)));
        corpus.push(format!("a{}", ".".repeat(n)));
    }
    for c in (0u8..=127).map(char::from) {
        corpus.push(c.to_string());
        corpus.push(format!("a{c}"));
        corpus.push(format!("{c}a"));
    }
    for s in &corpus {
        assert_eq!(
            re.is_match(s),
            is_valid_signup_source(s),
            "SOURCE_PATTERN and is_valid_signup_source disagree on {s:?}"
        );
    }
}

/// The text of the free function `name` in control.rs, from its signature to
/// the next column-0 item.
fn fn_body(name: &str) -> &'static str {
    let start = ["pub async fn ", "pub fn ", "fn "]
        .iter()
        .find_map(|p| CONTROL_RS.find(&format!("\n{p}{name}(")))
        .unwrap_or_else(|| panic!("control.rs has no fn {name}"));
    let rest = &CONTROL_RS[start + 1..];
    let end = rest.find("\n}\n").unwrap_or_else(|| panic!("fn {name} has no closing brace"));
    &rest[..end]
}

#[test]
fn signup_reads_its_arguments_only_through_the_signup_arg_consts() {
    // Every `pub const SIGNUP_ARG_*: &str = "<name>";` is a SIGNUP_ARGS row, and back.
    let re = Regex::new(r#"pub const SIGNUP_ARG_[A-Z_]+: &str = "([a-z_]+)";"#).unwrap();
    let consts: BTreeSet<String> = re.captures_iter(CONTROL_RS).map(|c| c[1].to_string()).collect();
    let declared: BTreeSet<String> = SIGNUP_ARGS.iter().map(|a| a.name.to_string()).collect();
    assert_eq!(consts, declared, "SIGNUP_ARG_* consts and SIGNUP_ARGS rows must be the same set");

    // No string-literal argument read inside signup / validate_source, and
    // each const is read by one of them.
    let literal = Regex::new(r#"(args\.get\(|arg_str\(args,\s*|arg_str_opt\(args,\s*|arg_bool\(args,\s*)"#).unwrap();
    let mut bodies = String::new();
    for f in ["signup", "validate_source"] {
        let body = fn_body(f);
        for m in literal.find_iter(body) {
            let after = &body[m.end()..];
            assert!(
                !after.starts_with('"'),
                "{f} reads an argument by string literal (use the SIGNUP_ARG_* const): {}",
                &body[m.start()..(m.end() + 20).min(body.len())]
            );
        }
        bodies.push_str(body);
    }
    for ident in ["SIGNUP_ARG_NAME", "SIGNUP_ARG_HANDOFF", "SIGNUP_ARG_SOURCE"] {
        assert!(bodies.contains(ident), "signup must read {ident}");
    }
}

#[test]
fn skill_files_carry_the_generated_signup_args_block() {
    let block = render_signup_args_block();
    assert!(block.starts_with(SIGNUP_ARGS_DOCS_START) && block.ends_with(SIGNUP_ARGS_DOCS_END));
    for a in SIGNUP_ARGS {
        assert!(block.contains(&format!("- `{}` (", a.name)), "block lists {}", a.name);
    }
    for (path, text) in [("plugin/skills/mcphost/SKILL.md", SKILL_MD), ("www/skill.md", WWW_SKILL_MD)] {
        assert_eq!(text.matches(SIGNUP_ARGS_DOCS_START).count(), 1, "{path}: one start marker");
        let s = text.find(SIGNUP_ARGS_DOCS_START).unwrap();
        let e = text.find(SIGNUP_ARGS_DOCS_END).expect("end marker") + SIGNUP_ARGS_DOCS_END.len();
        assert_eq!(&text[s..e], block, "{path}: signup args block differs from render_signup_args_block()");
    }
}
