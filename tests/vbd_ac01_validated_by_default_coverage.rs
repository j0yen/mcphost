//! PRD-mcphost-host-args-validated-by-default
//! AC1 (P0) -- Given `host_tool_descriptors(kinds)` + `admin_tool_descriptors()`,
//! When the generated coverage test runs, Then every descriptor carries
//! `x-mcphost-validated: true` except those named in `host_args::UNVALIDATED`,
//! the two sets partition the descriptors, every entry names a present tool and
//! an existing `tests/` file, the list is within its snapshot, and a planted
//! extra entry fails naming it.

use mcphost::handler::{admin_tool_descriptors, host_tool_descriptors};
use mcphost::host_args::{UNVALIDATED, UNVALIDATED_SNAPSHOT, VALIDATED_KEY};
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

fn descriptors() -> Vec<rmcp::model::Tool> {
    host_tool_descriptors(&KindRegistry::with_builtin())
        .into_iter()
        .chain(admin_tool_descriptors())
        .collect()
}

fn validated(tool: &rmcp::model::Tool) -> bool {
    tool.input_schema.get(VALIDATED_KEY) == Some(&Value::Bool(true))
}

/// The whole contract over an arbitrary opt-out list: `Err` names the offence.
fn check(tools: &[rmcp::model::Tool], unvalidated: &[(&str, &str)], snapshot: usize) -> Result<(), String> {
    let names: BTreeSet<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for (tool, reason) in unvalidated {
        if !names.contains(tool) {
            return Err(format!("UNVALIDATED entry `{tool}` names no descriptor"));
        }
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join(reason);
        if !file.is_file() {
            return Err(format!("UNVALIDATED entry `{tool}` reason `{reason}` is not a tests/ file"));
        }
    }
    let opted_out: BTreeSet<&str> = unvalidated.iter().map(|(t, _)| *t).collect();
    let mut missing = Vec::new();
    let mut validated_n = 0;
    for tool in tools {
        let is_validated = validated(tool);
        let is_out = opted_out.contains(tool.name.as_ref());
        match (is_validated, is_out) {
            (true, false) => validated_n += 1,
            (false, true) => {}
            (false, false) => missing.push(tool.name.to_string()),
            (true, true) => return Err(format!("`{}` is validated and also listed in UNVALIDATED", tool.name)),
        }
    }
    if !missing.is_empty() {
        return Err(format!("descriptors without {VALIDATED_KEY} and not in UNVALIDATED: {}", missing.join(", ")));
    }
    if validated_n + opted_out.len() != tools.len() {
        return Err(format!("{validated_n} validated + {} opted out != {} descriptors", opted_out.len(), tools.len()));
    }
    if unvalidated.len() > snapshot {
        let last = unvalidated.last().map(|(t, _)| *t).unwrap_or_default();
        return Err(format!("UNVALIDATED has {} entries, snapshot is {snapshot} (latest: `{last}`)", unvalidated.len()));
    }
    Ok(())
}

#[test]
fn vbd_ac01_every_descriptor_is_validated_unless_named_in_the_shrink_only_list() {
    let tools = descriptors();
    assert!(tools.len() >= 280, "only {} descriptors", tools.len());
    assert!(UNVALIDATED_SNAPSHOT <= 3, "the ceiling is 3");
    check(&tools, UNVALIDATED, UNVALIDATED_SNAPSHOT).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn vbd_ac01_a_planted_extra_entry_fails_naming_it() {
    let tools = descriptors();
    let mut planted: Vec<(&str, &str)> = UNVALIDATED.to_vec();
    planted.push(("host.whoami", "vbd_ac01_validated_by_default_coverage.rs"));
    let err = check(&tools, &planted, UNVALIDATED_SNAPSHOT).expect_err("a planted entry must fail");
    assert!(err.contains("host.whoami"), "{err}");

    let mut stale: Vec<(&str, &str)> = UNVALIDATED.to_vec();
    stale.push(("host.no_such_tool", "vbd_ac01_validated_by_default_coverage.rs"));
    let err = check(&tools, &stale, UNVALIDATED_SNAPSHOT + 1).expect_err("a stale entry must fail");
    assert!(err.contains("host.no_such_tool"), "{err}");
}
