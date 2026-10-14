//! PRD-mcphost-migration-refusal-contract AC3 (P0) -- Given
//! `--print-report-schema`, When compared to the committed
//! `docs/compat-report.schema.json`, Then they are byte-equal, and renaming
//! one field in a test copy of the struct makes the comparison fail.

use crate::mrefu;

use std::path::Path;
use std::process::Command;

use mcphost::compat_report::{Bypass, MigrationRef, TableChange};
use mrefu::mcphost_bin;
use schemars::JsonSchema;
use serde::Serialize;

fn committed_text() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/compat-report.schema.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} unreadable: {e}", path.display()))
}

fn pretty(schema: &schemars::Schema) -> String {
    let mut text = serde_json::to_string_pretty(schema).expect("schema serializes");
    text.push('\n');
    text
}

/// A copy of `CompatReport` with `from` renamed to `from_version`.
#[derive(Serialize, JsonSchema)]
#[allow(dead_code)]
struct CompatReport {
    schema_version: u32,
    ok: bool,
    from_version: String,
    to: String,
    db_path: String,
    checked_at_unix: i64,
    migrations: Vec<MigrationRef>,
    tables_changed: Vec<TableChange>,
    step: Option<String>,
    detail: Option<String>,
    reason: Option<String>,
    remedy: Option<String>,
    bypass: Option<Bypass>,
}

#[test]
fn mrefu_ac03_printed_schema_is_byte_equal_to_the_committed_file() {
    let out = Command::new(mcphost_bin())
        .arg("migrate")
        .arg("--print-report-schema")
        .output()
        .expect("run mcphost migrate --print-report-schema");
    assert!(out.status.success(), "print-report-schema exits 0");
    let printed = String::from_utf8(out.stdout).expect("schema is UTF-8");
    assert_eq!(
        printed,
        committed_text(),
        "docs/compat-report.schema.json drifted; regenerate with: mcphost migrate --print-report-schema > docs/compat-report.schema.json"
    );
    assert_eq!(
        mcphost::compat_report::report_schema_json(),
        printed,
        "the library function and the CLI print the same text"
    );
}

#[test]
fn mrefu_ac03_renaming_one_field_makes_the_comparison_fail() {
    let renamed = pretty(&schemars::schema_for!(CompatReport));
    let committed = committed_text();
    assert_ne!(renamed, committed, "a renamed field must not compare equal to the committed schema");

    let keys = |text: &str| -> Vec<String> {
        let doc: serde_json::Value = serde_json::from_str(text).expect("schema is JSON");
        doc["properties"].as_object().expect("properties").keys().cloned().collect()
    };
    let (kept, changed) = (keys(&committed), keys(&renamed));
    assert!(kept.contains(&"from".to_string()), "committed schema names `from`");
    assert!(
        changed.contains(&"from_version".to_string()) && !changed.contains(&"from".to_string()),
        "the copy renamed exactly that field: {changed:?}"
    );
}
