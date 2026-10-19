//! PRD-mcphost-tool-directory-format
//! AC2 (P0) — Given a directory whose `tool.toml` names a field the kind does
//! not accept, When `mcphost tool lint <dir>` runs, Then stdout carries
//! `invalid_spec` with the same message `check_unknown_spec_field` returns
//! and the exit code is 1.

use crate::common;
use mcphost::kinds::{check_unknown_spec_field, production_registry};
use serde_json::{Value, json};

#[test]
fn tdf_ac02_lint_prints_invalid_spec_with_the_servers_own_message_and_exits_1() {
    let scratch = common::TempDataDir::new();
    let dir = scratch.0.join("bad");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("tool.toml"),
        "[tool]\nname = \"bad_tool\"\nkind = \"echo\"\n\n[spec]\ndry_run = true\n\n[spec.schema]\ntype = \"object\"\n",
    )
    .unwrap();

    let registry = production_registry();
    let echo = registry.get("echo").unwrap();
    let expected = check_unknown_spec_field(echo.as_ref(), &json!({"schema": {"type": "object"}, "dry_run": true}), &registry)
        .expect("the server rejects dry_run on an echo spec")
        .to_string();
    assert!(expected.contains("'dry_run' is not a echo spec field"), "unexpected server message: {expected}");

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["tool", "lint"])
        .arg(&dir)
        .output()
        .expect("run mcphost tool lint");
    assert_eq!(out.status.code(), Some(1), "lint must exit 1 on a finding");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("invalid_spec"), "stdout must carry invalid_spec: {stdout}");
    let line: Value = serde_json::from_str(stdout.trim()).expect("stdout is one JSON line");
    assert_eq!(line["code"], json!("invalid_spec"));
    assert_eq!(line["message"], json!(expected), "message must be the server's own");
}
