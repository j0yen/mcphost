//! PRD-mcphost-recipe-golden-path-tests
//! AC5 -- Given every golden test, When `cargo test rgpt_` runs, Then each
//! recipe's exact calls execute in order, every `expect` holds, and no single
//! recipe exceeds 60 s.
//!
//! The golden tests are the generated `tests/rgpt_<recipe>_golden_path.rs`
//! files; `tests/support/rgpt.rs` runs them (steps in order, every `expect`,
//! `deadline_budget!` at `min(60 s, budget.wall_s)`). This file proves the
//! pieces that make `cargo test rgpt_` mean that: every recipe has a golden
//! test registered in a suite, it embeds exactly the toml's steps, and the
//! runner really runs in order and really fails on an `expect` that does not
//! hold.

use crate::rgpt;

use mcphost::recipes;
use serde_json::{Value, json};
use std::path::Path;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn embedded_spec(golden_source: &str) -> Value {
    let start = golden_source.find("r####\"").expect("raw string start") + "r####\"".len();
    let end = golden_source[start..].find("\"####;").expect("raw string end") + start;
    serde_json::from_str(&golden_source[start..end]).expect("embedded recipe json")
}

#[test]
fn rgpt_ac05_every_recipe_has_a_registered_golden_test_with_its_exact_steps() {
    let all = recipes::load_all(repo()).expect("load recipes");
    assert!(all.len() >= 7, "expected a toml per recipe, found {}", all.len());
    let suites: String = std::fs::read_dir(repo().join("tests"))
        .expect("tests dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("suite_core_") || n.starts_with("suite_sandbox_")))
        .map(|p| std::fs::read_to_string(p).expect("suite source"))
        .collect();
    for r in &all {
        let stem = format!("rgpt_{}_golden_path", r.name.replace('-', "_"));
        assert!(suites.contains(&format!("mod {stem};")), "{stem} is not a member of any suite");
        let source = std::fs::read_to_string(repo().join(format!("tests/{stem}.rs"))).expect("golden test file");
        assert!(source.contains(&format!("async fn {stem}()")), "{stem}: test fn missing");
        let spec = embedded_spec(&source);
        assert_eq!(spec, recipes::golden_spec(r), "{}: the golden test does not embed the toml's exact steps", r.name);
        let tools: Vec<&str> = spec["steps"].as_array().expect("steps").iter().map(|s| s["tool"].as_str().unwrap_or("signed_post")).collect();
        let want: Vec<&str> = r.steps.iter().map(|s| s.tool.as_deref().unwrap_or("signed_post")).collect();
        assert_eq!(tools, want, "{}: step order", r.name);
    }
}

fn chart_spec() -> Value {
    let chart = recipes::load_all(repo())
        .expect("load recipes")
        .into_iter()
        .find(|r| r.name == "chart-in-a-minute")
        .expect("chart-in-a-minute");
    recipes::golden_spec(&chart)
}

#[tokio::test]
async fn rgpt_ac05_runner_executes_the_calls_in_order() {
    let spec = chart_spec();
    let trace = rgpt::run(&spec).await;
    assert_eq!(trace.calls, ["host.table.create", "host.table.append", "host.table.chart"]);
    assert_eq!(trace.issued, 4, "one signup plus three steps");
}

#[tokio::test]
async fn rgpt_ac05_runner_fails_naming_the_step_when_an_expect_does_not_hold() {
    let mut spec = chart_spec();
    spec["steps"][2]["expect"] = json!([{"path": "$.schema", "value": "chart.v999", "contains": null, "exists": null}]);
    let outcome = tokio::spawn(async move { rgpt::run(&spec).await }).await;
    let err = outcome.expect_err("an expect that does not hold must fail the run");
    assert!(err.is_panic(), "{err}");
    let msg = err.into_panic().downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("chart-in-a-minute step 3") && msg.contains("host.table.chart") && msg.contains("chart.v999"), "{msg}");
}
