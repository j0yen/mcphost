//! PRD-mcphost-output-envelope-fields-generated AC3 -- Given the committed
//! `contracts/host-tools.v1.json`, When `mcphost contract dump` runs, Then its
//! output equals the committed file byte for byte, and `output_envelope_fields`
//! is sorted and holds the wrapper-added keys.

use serde_json::Value;
use std::process::Command;

const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

#[test]
fn mcphost_output_envelope_fields_generated_ac03() {
    let committed = std::fs::read(COMMITTED).expect("read committed contract");

    // The dump runs against a temp copy (it preserves `since` from the file
    // it overwrites), never the in-tree file.
    let dir = std::env::temp_dir().join(format!("mcphost-oef-ac03-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("host-tools.v1.json");
    std::fs::write(&copy, &committed).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["contract", "dump", "--path"])
        .arg(&copy)
        .output()
        .expect("run mcphost contract dump");
    assert!(out.status.success(), "dump failed: {}", String::from_utf8_lossy(&out.stderr));
    let dumped = std::fs::read(&copy).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(dumped == committed, "`mcphost contract dump` output differs from the committed file: run `mcphost contract dump`");

    let v: Value = serde_json::from_slice(&committed).unwrap();
    let fields: Vec<&str> = v["output_envelope_fields"]
        .as_array()
        .expect("output_envelope_fields array")
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    let mut sorted = fields.clone();
    sorted.sort_unstable();
    assert_eq!(fields, sorted, "output_envelope_fields must be sorted");
    for key in ["__resources_used", "call_id", "deprecations", "dry_run", "duration_ms"] {
        assert!(fields.contains(&key), "output_envelope_fields lacks `{key}`: {fields:?}");
    }
    // `state` is admitted: `host.tool.run`/`host.tool.test` stamp it (AC 1's
    // union), so the set carries it.
    assert!(fields.contains(&"state"), "AC 1's union admits `state`: {fields:?}");
}
