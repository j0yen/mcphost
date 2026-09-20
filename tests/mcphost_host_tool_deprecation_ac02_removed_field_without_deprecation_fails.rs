//! PRD-mcphost-host-tool-deprecation AC2 — Given a test build that removes
//! an argument from `host.tool_publish` with no deprecation entry, When
//! `cargo test` runs, Then the contract test fails naming the path.
//!
//! There is no second compiled binary to actually remove an argument
//! from -- this proves the mechanism `cargo test` runs on every real build
//! (`api_contract::diff`, wired into AC5's own removal test the same way)
//! by simulating the "before" side of that removal: a synthetic committed
//! contract with one extra field `host.tool_publish` no longer has,
//! diffed against the real live registry.

use mcphost::api_contract::{Violation, diff};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

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
