//! PRD-mcphost-output-envelope-fields-generated AC5 -- Given a temp copy of the
//! committed contract with one member removed from `output_envelope_fields`,
//! When `mcphost contract dump --check --path <copy>` runs, Then it exits
//! non-zero and its output names the missing member.

use serde_json::Value;
use std::process::Command;

const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

#[test]
fn mcphost_output_envelope_fields_generated_ac05() {
    let mut v: Value = serde_json::from_slice(&std::fs::read(COMMITTED).unwrap()).unwrap();
    let fields = v["output_envelope_fields"].as_array_mut().unwrap();
    let at = fields.iter().position(|f| f == "duration_ms").expect("duration_ms is a member");
    fields.remove(at);

    let dir = std::env::temp_dir().join(format!("mcphost-oef-ac05-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("host-tools.v1.json");
    std::fs::write(&copy, serde_json::to_vec_pretty(&v).unwrap()).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["contract", "dump", "--check", "--path"])
        .arg(&copy)
        .output()
        .expect("run mcphost contract dump --check");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success(), "--check must fail on a copy missing a member");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("duration_ms"), "output must name the missing member: {text}");
}
