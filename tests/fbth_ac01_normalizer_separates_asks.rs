//! PRD-mcphost-feedback-themes-nightly AC1 — Given the 59 live error texts of
//! the 10-08 chain stored as `fixtures/chain_errors_20261008.jsonl` with their
//! codes and tools, When `normalize` and the key are applied, Then the theme
//! count is between 4 and 40 (measured 28-31 on 10-13), proving the LLM-free
//! key separates asks. Also the R2 table test.

use mcphost::feedback::{normalize, theme_id_for_key, theme_key};
use serde_json::Value;
use std::collections::BTreeSet;

#[test]
fn chain_texts_separate_asks_without_exploding() {
    let raw = include_str!("fixtures/chain_errors_20261008.jsonl");
    let rows: Vec<Value> =
        raw.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).expect("jsonl row")).collect();
    assert_eq!(rows.len(), 59, "the fixture holds the chain's 59 live call errors");
    let s = |r: &Value, k: &str| r[k].as_str().unwrap().to_string();
    // Mapping per the fixture's .md: text = result, tool = the called MCP tool (`tool`; `args.name` is a per-tenant tool name), code = failure_class.
    let tool = |r: &Value| r["tool"].as_str().unwrap().to_string();
    let themes: BTreeSet<String> = rows
        .iter()
        .map(|r| {
            theme_id_for_key(&theme_key("error", &s(r, "failure_class"), &tool(r), &s(r, "result")))
        })
        .collect();
    let listing = rows
        .iter()
        .map(|r| format!("{} | {}", tool(r), normalize(&s(r, "result"))))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("\n");
    // AC1 bound; the falsifier is 1 theme (cannot separate) or one per row (cannot merge).
    assert!((4..=40).contains(&themes.len()), "{} themes:\n{listing}", themes.len());
    // Every row shares one failure_class, so the text part of the key must be doing
    // real work: more themes than distinct tools, far fewer than distinct raw texts.
    let tools: BTreeSet<String> = rows.iter().map(tool).collect();
    let raw_texts: BTreeSet<String> = rows.iter().map(|r| s(r, "result")).collect();
    assert!(themes.len() > tools.len(), "{} themes vs {} tools", themes.len(), tools.len());
    assert!(themes.len() < raw_texts.len(), "{} themes vs {} raw texts", themes.len(), raw_texts.len());
}

#[test]
fn normalize_table() {
    let cases: [(&str, &str); 26] = [
        ("args_invalid said 4.78 is not an integer", "args_invalid _ not integer"),
        ("args_invalid says 12.5 is not an integer", "args_invalid _ not integer"),
        ("Schema SAID number", "schema number"),
        ("", ""),
        ("   ", ""),
        ("12345", ""),
        ("rejected \"abc def\" as invalid", "rejected _ as invalid"),
        ("rejected 'abc def' as invalid", "rejected _ as invalid"),
        ("rejected `abc` as invalid", "rejected _ as invalid"),
        ("it doesn't work", "doesn t work"),
        ("feedback f_3a91bc42 was lost", "feedback _ lost"),
        ("theme th_0123456789ab is stale", "theme _ stale"),
        ("tool t_5521ab missing", "tool _ missing"),
        ("request 9f3a0c11d2e4 failed", "request _ failed"),
        ("failed after 30 seconds", "failed after _ seconds"),
        ("one two three four five six seven eight", "one two three four five six"),
        ("a b c the of", "b c"),
        ("call_timeout after 30s", "call_timeout after"),
        ("v2 api is gone", "_ api gone"),
        ("Ünïcode ünï here", "_ here"),
        ("KeyError: 0 at tool.py:41", "keyerror"),
        ("null is not of type \"string\"", "_ not type"),
        ("tool x at home", "tool x home"),
        ("{\"body\":\"a \\\"b\\\" c\"} is not of type \"string\"", "_ not type"),
        ("tool not found: t_6238451c.lc_chain_runner", "tool not found"),
        ("budget.max_tool_latency_ms: 180000 exceeds", "_ exceeds"),
    ];
    for (input, want) in cases {
        assert_eq!(normalize(input), want, "normalize({input:?})");
    }
}

#[test]
fn tool_part_drops_tenant_hex_suffix() {
    let a = theme_key("error", "host_refusal", "lambda_cost_watch_358f31f4", "boom");
    let b = theme_key("error", "host_refusal", "lambda_cost_watch", "boom");
    let c = theme_key("error", "host_refusal", "dc617cce_transform", "boom");
    assert_eq!(a, b);
    assert_eq!(c, "error|host_refusal|transform|boom");
}
