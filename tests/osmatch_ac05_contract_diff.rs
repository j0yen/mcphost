//! contract: reads registry handler::host_tool_descriptors
//! contract: reads contracts/host-output-observational-edits.v1.json
//! PRD-mcphost-success-result-matches-output-schema
//! AC5 (P0) -- Given the composed registry, When `api_contract::diff` compares
//! it with the contract at the cited tree, Then it reports zero removed fields
//! and zero narrowed types with the five `would` properties exempt because they
//! moved into the dry-run branch, every other finding is widened or a dropped
//! `required` key named by an R4 entry, and every root stays `type: object`
//! within the `outsch_ac01` union budget.

use std::collections::BTreeSet;

use mcphost::api_contract::{Violation, diff, dump_contract};
use mcphost::kinds::KindRegistry;
use mcphost::output_schemas::{EditKind, max_union_branches, observational_edits, output_schema_for, registered_tools};
use serde_json::{Value, json};

const WOULD_TOOLS: [&str; 5] =
    ["host.invite.create", "host.oauth.policy_set", "host.state.delete_rows", "host.table.create", "host.table.drop"];

fn cited_contract() -> Value {
    serde_json::from_str(include_str!("fixtures/osmatch/cited-output-schemas.json")).expect("fixture")
}

fn cited_schema(tool: &str) -> Value {
    cited_contract()["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == tool)
        .map(|t| t["output_schema"].clone())
        .unwrap_or_else(|| panic!("{tool} in the cited tree"))
}

#[test]
fn the_diff_reports_nothing_removed_nothing_narrowed_and_only_the_named_dropped_required_keys() {
    let live = dump_contract(&KindRegistry::with_builtin());
    let findings = diff(&cited_contract(), &live, &[], mcphost::state::now_unix());

    let mut dropped = BTreeSet::new();
    let mut problems = Vec::new();
    for finding in &findings {
        match finding {
            Violation::Narrowed { path, detail } if detail == "no longer always present" => {
                dropped.insert(path.clone());
            }
            // PRD-mcphost-tool-call-output-root-opaque R1: the forwarding roots
            // dropped their sampled keys on purpose.
            Violation::OpaqueRoot { path, .. }
                if mcphost::output_schemas::FORWARDING_ROOTS.iter().any(|t| path == &format!("{t}.output")) => {}
            other => problems.push(format!("{other:?}")),
        }
    }
    assert!(problems.is_empty(), "removed or narrowed: {}", problems.join("\n"));

    let named: BTreeSet<String> = observational_edits()
        .entries
        .iter()
        .filter(|e| e.edit == EditKind::DropRequired)
        .map(|e| format!("{}.output.{}", e.tool, e.pointer.rsplit('/').next().unwrap_or_default()))
        .collect();
    assert_eq!(dropped, named, "a dropped `required` key is reported exactly when an R4 entry names it");
}

#[test]
fn the_five_would_properties_moved_into_the_dry_run_branch() {
    for tool in WOULD_TOOLS {
        assert!(cited_schema(tool)["properties"].get("would").is_some(), "{tool} carried `would` at the cited tree");
        let now = Value::Object((*output_schema_for(tool).expect("registered")).clone());
        assert!(now["properties"].get("would").is_none(), "{tool}: the live properties no longer carry `would`");
        let branch = now["anyOf"].as_array().expect("anyOf").iter().find(|b| b["properties"].get("would").is_some());
        assert!(branch.is_some(), "{tool}: `would` lives in the dry-run branch");
    }
}

#[test]
fn a_narrowed_type_and_a_removed_field_are_still_reported() {
    let old = json!({"tools": [{"name": "t", "input_schema": {}, "output_schema":
        {"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": "integer"}}}}]});
    let new = json!({"tools": [{"name": "t", "input_schema": {}, "output_schema":
        {"type": "object", "properties": {"a": {"type": "integer"}}}}]});
    let findings = diff(&old, &new, &[], 0);
    assert!(findings.iter().any(|f| matches!(f, Violation::Removed { path } if path == "t.output.b")), "{findings:?}");
    assert!(findings.iter().any(|f| matches!(f, Violation::Narrowed { path, .. } if path == "t.output.a")), "{findings:?}");
    let widened = json!({"tools": [{"name": "t", "input_schema": {}, "output_schema":
        {"type": "object", "properties": {"a": {"type": ["string", "null"]}, "b": {"type": "integer"}}}}]});
    assert!(diff(&old, &widened, &[], 0).is_empty(), "gaining `null` is a widening");
}

#[test]
fn every_root_is_an_object_within_the_union_budget() {
    let tools = registered_tools();
    let mut over_budget = Vec::new();
    for tool in &tools {
        let schema = Value::Object((*output_schema_for(tool).expect("registered")).clone());
        assert_eq!(schema.get("type"), Some(&json!("object")), "{tool}: root must stay `type: object`");
        let branches = max_union_branches(&schema);
        if branches > 3 {
            over_budget.push(format!("{tool} ({branches})"));
        }
    }
    assert!(over_budget.len() <= 18, "outsch_ac01 budget: {} tools over 3 branches: {over_budget:?}", over_budget.len());
    assert!(over_budget.len() <= tools.len() / 10, "{over_budget:?}");
}
