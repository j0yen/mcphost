//! PRD-mcphost-host-tool-deprecation AC2 — Given a test build that removes
//! an argument from `host.tool_publish` with no deprecation entry, When
//! `cargo test` runs, Then the contract test fails naming the path.
//!
//! There is no second compiled binary to actually remove an argument
//! from -- this proves the mechanism `cargo test` runs on every real build
//! (`api_contract::diff`, wired into AC5's own removal test the same way).
//! `committed_contract` below reads the exact bytes of the real,
//! committed `contracts/host-tools.v1.json` (via `include_bytes!`, the
//! same cwd-independence trick AC1's own test already uses -- see its
//! comment) rather than a value built in memory, so
//! `live_registry_matches_the_committed_contract` and
//! `removing_a_field_the_committed_contract_has_fails_naming_the_path`
//! below genuinely diff the live registry against that committed file,
//! not a synthetic stand-in for it. `bare_removal_with_no_deprecation_entry_fails_naming_the_path`
//! and `additions_never_produce_a_violation` keep exercising the same
//! mechanism with a synthetic fake field, for the fields the real
//! committed contract doesn't happen to cover.

use mcphost::api_contract::{Violation, diff};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

/// The real, committed contract this PRD's `diff` is meant to guard --
/// same file `DEFAULT_CONTRACT_PATH` names at runtime. Read at compile
/// time (like AC1's own `COMMITTED_CONTRACT`) so the test proves the
/// actual checked-in baseline regardless of the test binary's cwd.
const COMMITTED_CONTRACT_BYTES: &[u8] = include_bytes!("../contracts/host-tools.v1.json");

fn committed_contract() -> Value {
    serde_json::from_slice(COMMITTED_CONTRACT_BYTES).expect("committed contract parses as JSON")
}

/// The path AC2/AC3/AC5 all exercise as "the argument that gets removed
/// from `host.tool_publish`" -- a fake field injected only into the
/// synthetic "old"/committed side, never into the real live registry.
const REMOVED_PATH: &str = "host.tool_publish.legacy_flag";

fn find_tool_mut<'a>(contract: &'a mut Value, name: &str) -> &'a mut Value {
    contract["tools"]
        .as_array_mut()
        .expect("tools array")
        .iter_mut()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("{name} must be in the dumped contract"))
}

/// A synthetic "old" contract: the real live dump, with one extra
/// optional field spliced into `host.tool_publish`'s properties -- as if
/// that field existed when this was committed and has since been removed.
fn live_contract_with_extra_field() -> Value {
    let mut contract = mcphost::api_contract::dump_contract(&KindRegistry::with_builtin());
    let tool = find_tool_mut(&mut contract, "host.tool_publish");
    tool["input_schema"]["properties"]["legacy_flag"] = json!({
        "type": "string",
        "description": "a since-removed argument, for this test only",
    });
    contract
}

#[test]
fn bare_removal_with_no_deprecation_entry_fails_naming_the_path() {
    let old = live_contract_with_extra_field();
    let live = mcphost::api_contract::dump_contract(&KindRegistry::with_builtin());

    let violations = diff(&old, &live, &[], mcphost::state::now_unix());

    let found = violations
        .iter()
        .find(|v| v.path() == REMOVED_PATH)
        .unwrap_or_else(|| {
            panic!(
                "expected a violation naming {REMOVED_PATH}, got: {violations:?}"
            )
        });
    assert!(
        matches!(found, Violation::Removed { .. }),
        "a removal with zero deprecation entries must be Violation::Removed, got {found:?}"
    );
    assert!(
        found.message().contains(REMOVED_PATH),
        "the violation message must name the path: {}",
        found.message()
    );
}

#[test]
fn live_registry_matches_the_committed_contract() {
    // Sanity precondition for the next test: today's live registry has
    // not drifted from the real committed baseline, so any violation
    // `diff` reports there can only come from the removal we inject.
    let committed = committed_contract();
    let live = mcphost::api_contract::dump_contract(&KindRegistry::with_builtin());
    let violations = diff(&committed, &live, &[], mcphost::state::now_unix());
    assert!(
        violations.is_empty(),
        "the live registry must match the committed contract with no deprecations in play: {violations:?}"
    );
}

#[test]
fn removing_a_field_the_committed_contract_has_fails_naming_the_path() {
    // Unlike `bare_removal_with_no_deprecation_entry_fails_naming_the_path`
    // above (a fake field spliced only into an in-memory "old"), `old`
    // here IS the real committed file, and `live` is that same file with
    // one field an actual PR could remove -- `host.tool_publish.tenant_key`
    // -- deleted. This is what a real removal without a
    // contracts/deprecations.json entry looks like against the actual
    // shipped contract, not a synthetic stand-in for it.
    let old = committed_contract();
    let mut live = committed_contract();
    let tool = find_tool_mut(&mut live, "host.tool_publish");
    tool["input_schema"]["properties"]
        .as_object_mut()
        .expect("properties object")
        .remove("tenant_key")
        .expect("host.tool_publish.tenant_key must exist in the committed contract for this test");

    let path = "host.tool_publish.tenant_key";
    let violations = diff(&old, &live, &[], mcphost::state::now_unix());
    let found = violations
        .iter()
        .find(|v| v.path() == path)
        .unwrap_or_else(|| panic!("expected a violation naming {path}, got: {violations:?}"));
    assert!(
        matches!(found, Violation::Removed { .. }),
        "a removal with zero deprecation entries must be Violation::Removed, got {found:?}"
    );
    assert!(
        found.message().contains(path),
        "the violation message must name the path: {}",
        found.message()
    );
}

#[test]
fn additions_never_produce_a_violation() {
    // The opposite direction of the same mechanism (requirement 2:
    // "additions pass"): the committed side is the real (smaller)
    // registry, live has the extra field -- diff must find nothing wrong.
    let committed = mcphost::api_contract::dump_contract(&KindRegistry::with_builtin());
    let live_with_addition = live_contract_with_extra_field();

    let violations = diff(&committed, &live_with_addition, &[], mcphost::state::now_unix());
    assert!(
        violations.is_empty(),
        "an added field must never be a violation: {violations:?}"
    );
}
