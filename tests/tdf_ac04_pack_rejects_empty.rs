//! PRD-mcphost-tool-directory-format
//! AC4 (P0) — Given an empty directory or a `tool.toml` with no `[tool]`
//! table, When `pack` runs, Then it exits 1 with `invalid_spec` and writes
//! nothing.

use crate::common;
use serde_json::{Value, json};
use std::path::Path;

fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn pack_cli(dir: &Path) -> (Option<i32>, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["tool", "pack"])
        .arg(dir)
        .output()
        .expect("run mcphost tool pack");
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

fn assert_rejected(dir: &Path, before: Vec<String>) {
    let (code, stdout) = pack_cli(dir);
    assert_eq!(code, Some(1), "pack must exit 1: {stdout}");
    let line: Value = serde_json::from_str(stdout.trim()).expect("one JSON line on stdout");
    assert_eq!(line["code"], json!("invalid_spec"), "{stdout}");
    assert_eq!(listing(dir), before, "pack must write nothing");
}

#[test]
fn tdf_ac04_empty_directory_is_invalid_spec_and_writes_nothing() {
    let scratch = common::TempDataDir::new();
    let dir = scratch.0.join("empty");
    std::fs::create_dir_all(&dir).unwrap();
    assert_rejected(&dir, Vec::new());
}

#[test]
fn tdf_ac04_tool_toml_without_tool_table_is_invalid_spec_and_writes_nothing() {
    let scratch = common::TempDataDir::new();
    let dir = scratch.0.join("no_tool_table");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("tool.toml"), "[spec]\nschema = { type = \"object\" }\n").unwrap();
    assert_rejected(&dir, vec!["tool.toml".to_string()]);
}
