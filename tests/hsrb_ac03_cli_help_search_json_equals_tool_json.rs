//! PRD-mcphost-help-search-tool-b
//! AC3 (P1) — Given `mcphost help-search "network public" --json`, When run,
//! Then stdout equals the tool's JSON for the same query.

use mcphost::examples::registered_kinds;
use serde_json::{Value, json};

#[test]
fn hsrb_ac03_cli_json_stdout_equals_the_tool_json() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["help-search", "network public", "--json"])
        .output()
        .expect("run mcphost help-search");
    assert!(out.status.success(), "exit {:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    let stdout: Value = serde_json::from_slice(&out.stdout).expect("stdout is one JSON document");

    let tool = mcphost::help_search::call(&registered_kinds(), &json!({"q": "network public"})).expect("tool call");
    assert!(!tool["hits"].as_array().expect("hits").is_empty(), "the query must have hits: {tool}");
    assert_eq!(stdout, tool);
}

#[test]
fn hsrb_ac03_cli_kind_flag_matches_the_tool_kind_argument() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["help-search", "import", "--kind", "python", "--json"])
        .output()
        .expect("run mcphost help-search");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout: Value = serde_json::from_slice(&out.stdout).unwrap();
    let tool = mcphost::help_search::call(&registered_kinds(), &json!({"q": "import", "kind": "python"})).unwrap();
    assert_eq!(stdout, tool);
}
