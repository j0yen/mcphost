//! PRD-mcphost-contract-output-schemas
//! AC1 (P0) -- Given the responses captured by the suite for every canonical
//! host tool, When each tool's rows are checked against its declared schema,
//! Then <= 10 % of tools need a `oneOf` of more than 3 branches; the test
//! prints the over-budget tools and fails the PRD's premise if the count
//! exceeds 18.

use crate::common;

use mcphost::output_schemas::{max_union_branches, output_schema_for, registered_tools};
use serde_json::Value;

#[test]
fn at_most_ten_percent_of_tools_need_a_oneof_of_more_than_three_branches() {
    let tools = registered_tools();
    assert!(!tools.is_empty(), "no output schemas registered");
    let rows = common::outsch_rows();

    let mut over_budget = Vec::new();
    let mut failures = Vec::new();
    for tool in &tools {
        let schema = Value::Object((*output_schema_for(tool).expect("registered")).clone());
        let branches = max_union_branches(&schema);
        if branches > 3 {
            over_budget.push(format!("{tool} ({branches} branches)"));
        }
        let validator = jsonschema::validator_for(&schema).unwrap_or_else(|e| panic!("{tool}: schema invalid: {e}"));
        for row in rows.get(*tool).into_iter().flatten() {
            if let Err(e) = validator.validate(row) {
                failures.push(format!("{tool}: {e}"));
            }
        }
    }
    println!("tools needing a oneOf of more than 3 branches ({}): {over_budget:?}", over_budget.len());
    assert!(failures.is_empty(), "captured rows rejected by their schema:\n{}", failures.join("\n"));

    let allowed = tools.len() / 10;
    assert!(
        over_budget.len() <= 18,
        "premise falsified: {} tools need a oneOf of more than 3 branches (limit 18): {over_budget:?}",
        over_budget.len()
    );
    assert!(
        over_budget.len() <= allowed,
        "{} of {} tools exceed the 3-branch budget (10 % = {allowed}): {over_budget:?}",
        over_budget.len(),
        tools.len()
    );
}

#[test]
fn the_corpus_covers_at_least_eighty_percent_of_registered_tools() {
    let tools = registered_tools();
    let rows = common::outsch_rows();
    let covered = tools.iter().filter(|t| rows.get(**t).is_some_and(|r| !r.is_empty())).count();
    let uncovered: Vec<&&str> = tools.iter().filter(|t| rows.get(**t).is_none_or(|r| r.is_empty())).collect();
    println!("coverage {covered}/{}; uncovered: {uncovered:?}", tools.len());
    assert!(covered * 10 >= tools.len() * 8, "coverage {covered}/{} < 80 %: {uncovered:?}", tools.len());
}
