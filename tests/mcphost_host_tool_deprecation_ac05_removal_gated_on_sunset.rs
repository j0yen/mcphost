//! PRD-mcphost-host-tool-deprecation AC5 — Given a deprecation whose
//! sunset has not passed, When the field is removed, Then the contract
//! test fails; when the sunset has passed, Then it passes.

use mcphost::api_contract::{Deprecation, Violation, diff, dump_contract};
use mcphost::kinds::KindRegistry;
use mcphost::state::parse_date_ymd_unix;
use serde_json::Value;

const REMOVED_PATH: &str = "host.tool_publish.legacy_flag";

fn find_tool_mut<'a>(contract: &'a mut Value, name: &str) -> &'a mut Value {
    contract["tools"]
        .as_array_mut()
        .expect("tools array")
        .iter_mut()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("{name} must be in the dumped contract"))
}

/// Same synthetic "old" contract shape as AC2's own test: the live dump
/// with one extra field spliced into `host.tool_publish`, standing in for
/// a field that has now actually been removed from `live`.
fn old_contract_with_extra_field() -> Value {
    let mut contract = dump_contract(&KindRegistry::with_builtin());
    let tool = find_tool_mut(&mut contract, "host.tool_publish");
    tool["input_schema"]["properties"]["legacy_flag"] = serde_json::json!({
        "type": "string",
        "description": "a since-removed argument, for this test only",
    });
    contract
}

fn entry_with_sunset(sunset: &str) -> Deprecation {
    Deprecation {
        path: REMOVED_PATH.to_string(),
        since: "2026-01-01".to_string(),
        sunset: sunset.to_string(),
        replacement: "host.tool_publish.kind".to_string(),
    }
}

#[test]
fn removal_before_sunset_fails() {
    let old = old_contract_with_extra_field();
    let live = dump_contract(&KindRegistry::with_builtin());
    // Valid 60-day lead time, but far in the future relative to "today".
    let entry = entry_with_sunset("2026-03-02");
    assert!(entry.lead_time_valid());

    // "today" is well before the sunset date -- the field was pulled too
    // early.
    let today = parse_date_ymd_unix("2026-02-01").expect("valid fixture date");
    let violations = diff(&old, &live, &[entry], today);

    let found = violations
        .iter()
        .find(|v| v.path() == REMOVED_PATH)
        .unwrap_or_else(|| panic!("expected a violation naming {REMOVED_PATH}: {violations:?}"));
    assert!(
        matches!(found, Violation::RemovedBeforeSunset { .. }),
        "removing before sunset must be RemovedBeforeSunset, got {found:?}"
    );
    assert!(found.message().contains("2026-03-02"), "{}", found.message());
}

#[test]
fn removal_after_sunset_passes() {
    let old = old_contract_with_extra_field();
    let live = dump_contract(&KindRegistry::with_builtin());
    let entry = entry_with_sunset("2026-03-02");

    // "today" is on the sunset date itself.
    let today = parse_date_ymd_unix("2026-03-02").expect("valid fixture date");
    let violations = diff(&old, &live, &[entry], today);
    assert!(
        violations.is_empty(),
        "removal on/after a valid entry's sunset must not violate: {violations:?}"
    );
}
