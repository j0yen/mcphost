//! contract: reads registry handler::host_tool_descriptors
//! contract: reads contracts/host-output-schemas.v1.json
//! PRD-mcphost-success-result-matches-output-schema
//! AC1 (P0) -- Given every `WRITE_VERBS` entry with `DryRun::Supported` and a
//! non-`Publish` kind, When `build_would` output (zero or one note, with
//! `confirm` and `_meta.scratch`) is validated against the verb's `tools/list`
//! `outputSchema`, Then all 44 validate, the branch is one fn in
//! `src/write_verbs.rs`, the contract file has no `"would"`, and
//! `scripts/sandbox-api-doc-check.sh` parses the `sv(`/`nd(` rows unchanged.

use std::path::Path;
use std::process::Command;

use crate::common;
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::write_verbs::{DRY_RUN_VERBS, DryRun, Kind, NO_DRY_RUN_VERBS, WRITE_VERBS, build_would};
use serde_json::{Value, json};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn dry_run_verbs() -> Vec<&'static mcphost::write_verbs::WriteVerb> {
    WRITE_VERBS
        .iter()
        .filter(|v| matches!(v.dry_run, DryRun::Supported) && v.kind != Kind::Publish)
        .collect()
}

#[test]
fn every_generic_dry_run_preview_validates_against_the_advertised_output_schema() {
    let verbs = dry_run_verbs();
    assert_eq!(verbs.len(), 44, "45 `sv(` rows minus the Publish one");
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let mut problems = Vec::new();
    for verb in verbs {
        let tool = tools
            .iter()
            .find(|t| t.name.as_ref() == verb.name)
            .unwrap_or_else(|| panic!("{} missing from tools/list", verb.name));
        let schema = Value::Object((**tool.output_schema.as_ref().expect("outputSchema")).clone());
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        let one_note = [("tables", json!({"action": "drop", "name": "t", "rows": 3, "bytes": 9}))];
        let plain = build_would(verb, &json!({}), &[]);
        let noted = build_would(verb, &json!({}), &one_note);
        let mut confirmed = noted.clone();
        confirmed["confirm"] = json!("mcp_confirm_token");
        let mut scratch = plain.clone();
        scratch["_meta"]["scratch"] = json!(true);
        for (label, row) in [("plain", plain), ("one note", noted), ("confirm", confirmed), ("scratch", scratch)] {
            if let Some(e) = validator.iter_errors(&row).next() {
                problems.push(format!("{} {label}: {}: {e}", verb.name, e.instance_path));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_branch_is_one_fn_and_the_contract_file_has_no_would() {
    let src = std::fs::read_to_string(root().join("src/write_verbs.rs")).expect("write_verbs.rs");
    assert_eq!(src.matches("fn dry_run_output_branch").count(), 1);
    let contract = std::fs::read_to_string(root().join("contracts/host-output-schemas.v1.json")).expect("contract");
    assert!(!contract.contains("\"would\""), "the five `would` properties moved into the dry-run branch");
}

#[test]
fn the_doc_check_script_still_parses_the_sv_and_nd_rows() {
    let src = std::fs::read_to_string(root().join("src/write_verbs.rs")).expect("write_verbs.rs");
    let rows = |prefix: &str| src.lines().filter(|l| l.trim_start().starts_with(prefix) && l.trim_end().ends_with("),")).count();
    assert_eq!(
        (rows("sv(\""), rows("nd(\"")),
        (DRY_RUN_VERBS.len(), NO_DRY_RUN_VERBS.len()),
        "every registry row is still one `sv(`/`nd(` line"
    );
    let dir = common::doc_check_scratch("osmatch-ac01");
    let out = Command::new("bash")
        .arg(dir.join("scripts/sandbox-api-doc-check.sh"))
        .output()
        .expect("run script");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}
