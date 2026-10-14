//! PRD-mcphost-tool-idempotent-declaration AC7 -- Given `docs/tools.md` and
//! `www/llms.txt`, When `mcphost llms-txt --check` runs, Then both generated
//! sections are current and the command exits 0.

use crate::common;
use mcphost::handler::{ANNOTATION_FIELDS, HOST_TOOL_HINTS};
use std::process::Command;

fn repo(path: &str) -> String {
    std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn llms_txt_check(extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["llms-txt", "--check"])
        .args(extra)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mcphost llms-txt --check")
}

#[test]
fn check_exits_zero_and_covers_both_generated_sections() {
    let out = llms_txt_check(&[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout} stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("docs/tools.md is up to date"), "--check must cover docs/tools.md: {stdout}");
    assert!(stdout.contains("www/llms.txt is up to date"), "{stdout}");

    // The sections themselves: the three field names in llms.txt, one table
    // row per HOST_TOOL_HINTS entry in docs/tools.md.
    let llms = repo("www/llms.txt");
    for field in ANNOTATION_FIELDS {
        assert!(llms.contains(&format!("`{field}`")), "www/llms.txt names {field}");
    }
    let tools = repo("docs/tools.md");
    assert!(tools.contains("## Annotations"));
    for (name, idempotent, read_only) in HOST_TOOL_HINTS {
        let row = format!("| `{name}` | {idempotent} | {read_only} | {} |", !idempotent);
        assert!(tools.contains(&row), "docs/tools.md Annotations row missing: {row}");
    }
}

#[test]
fn check_fails_when_the_llms_txt_annotation_line_is_stale() {
    let dir = common::TempDataDir::new();
    let stale = repo("www/llms.txt")
        .lines()
        .filter(|l| !l.contains("carries the MCP annotations"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let path = dir.0.join("llms.txt");
    std::fs::write(&path, stale).unwrap();
    let out = llms_txt_check(&["--path", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "stale llms.txt must exit 1: {}", String::from_utf8_lossy(&out.stderr));
}
