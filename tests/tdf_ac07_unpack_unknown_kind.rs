//! PRD-mcphost-tool-directory-format
//! AC7 (P2) — Given a payload with `kind: "message"`, When `unpack` runs,
//! Then it writes nothing and prints `unknown_kind` followed by the
//! registry's names.

use crate::common;
use mcphost::kinds::production_registry;
use serde_json::json;

#[test]
fn tdf_ac07_unknown_kind_writes_nothing_and_names_the_registry() {
    let scratch = common::TempDataDir::new();
    let payload = scratch.0.join("payload.json");
    let target = scratch.0.join("out");
    std::fs::write(
        &payload,
        json!({"name": "msg_tool", "kind": "message", "spec": {"schema": {"type": "object"}}}).to_string(),
    )
    .unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["tool", "unpack"])
        .arg(&payload)
        .arg(&target)
        .output()
        .expect("run mcphost tool unpack");
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let at = stdout.find("unknown_kind").unwrap_or_else(|| panic!("stdout must carry unknown_kind: {stdout}"));
    let names = production_registry().names();
    assert!(!names.is_empty());
    let mut from = at;
    for name in &names {
        let pos = stdout[from..].find(name).unwrap_or_else(|| panic!("registry name {name} must follow unknown_kind: {stdout}"));
        from += pos;
    }
    assert!(!target.exists(), "unpack must not create the target directory");
}
