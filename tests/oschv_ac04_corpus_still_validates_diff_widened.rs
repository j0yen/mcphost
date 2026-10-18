//! PRD-mcphost-output-schema-tenant-values-opaque
//! AC4 (P0) -- Given the captured-row corpus under the outsch fixtures, When
//! validated after the widening, Then every row still validates and the
//! contract diff against the pre-PRD contract reports the opaque nodes as
//! `widened` and nothing as `removed`.

use crate::common;
use mcphost::api_contract::{Violation, diff, dump_contract, widened};
use mcphost::kinds::KindRegistry;
use mcphost::output_schemas::{OPAQUE_FIELDS, output_schema_for};
use serde_json::{Value, json};

#[test]
fn every_captured_row_still_validates_against_the_widened_registry() {
    let rows = common::outsch_rows();
    assert!(!rows.is_empty(), "no captured rows under tests/fixtures/outsch");
    let mut problems = Vec::new();
    for (tool, rows) in &rows {
        let Some(schema) = output_schema_for(tool) else { continue };
        let validator = jsonschema::validator_for(&Value::Object((*schema).clone())).expect("schema compiles");
        for (i, row) in rows.iter().enumerate() {
            if let Some(e) = validator.iter_errors(row).next() {
                problems.push(format!("{tool} row {i}: {}: {e}", e.instance_path));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The pre-PRD contract: the live dump with every opaque node put back to the
/// typed sample the old registry carried.
fn pre_prd_contract(live: &Value) -> Value {
    let mut old = live.clone();
    for (tool, pointer) in OPAQUE_FIELDS {
        let entry = old["tools"]
            .as_array_mut()
            .expect("tools")
            .iter_mut()
            .find(|t| t["name"] == *tool)
            .unwrap_or_else(|| panic!("{tool} in contract"));
        *entry["output_schema"]
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("{tool} {pointer} in output_schema")) = json!({"type": "string"});
    }
    old
}

#[test]
fn the_diff_against_the_pre_prd_contract_reports_widened_and_never_removed() {
    let live = dump_contract(&KindRegistry::with_builtin());
    let old = pre_prd_contract(&live);

    let violations = diff(&old, &live, &[], mcphost::state::now_unix());
    assert!(
        !violations.iter().any(|v| matches!(v, Violation::Removed { .. } | Violation::RemovedBeforeSunset { .. })),
        "a widening must not read as a removal: {violations:?}"
    );
    assert!(violations.is_empty(), "{violations:?}");

    let report = widened(&old, &live);
    for expected in [
        "host.state.get.output.value",
        "host.tool.call.output.steps[].result",
        "host.runs.get.output.result",
    ] {
        assert!(report.iter().any(|p| p == expected), "{expected} not reported widened: {report:?}");
    }
    assert!(
        report.len() >= OPAQUE_FIELDS.len(),
        "{} opaque nodes but {} reported: {report:?}",
        OPAQUE_FIELDS.len(),
        report.len()
    );
    assert!(widened(&live, &live).is_empty(), "an unchanged contract widens nothing");
}
