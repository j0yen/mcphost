//! PRD-mcphost-recipe-golden-path-tests
//! AC7 -- Given a PR that breaks one recipe step (fixture route renamed),
//! When CI runs, Then the `recipes-golden-path` job fails on that recipe only
//! and the golden run finished within 6 minutes.
//!
//! What a test can prove without a CI run: the job exists, runs every `rgpt_`
//! test on every PR, times its golden RUN step at 6 minutes (the job cap
//! covers a cold compile too) and has no `needs:`; and a renamed
//! fixture route makes that recipe's golden path fail while another recipe's
//! still passes. The wall time of a real runner is the PRD's AC8-style live
//! evidence and is read from the first CI run after landing.

use crate::rgpt;

use mcphost::recipes;
use serde_json::Value;
use std::path::Path;

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The text of the `recipes-golden-path` job: its key line up to the next
/// job key (a line indented exactly two spaces that ends in `:`).
fn job_text() -> String {
    let yaml = std::fs::read_to_string(repo().join(".github/workflows/ci.yml")).expect("ci.yml");
    let lines: Vec<&str> = yaml.lines().collect();
    let start = lines.iter().position(|l| *l == "  recipes-golden-path:").expect("a recipes-golden-path job in ci.yml");
    let is_job_key = |l: &&str| l.starts_with("  ") && !l.starts_with("   ") && l.trim_end().ends_with(':') && !l.trim_start().starts_with('#');
    let end = lines[start + 1..].iter().position(is_job_key).map_or(lines.len(), |p| p + start + 1);
    lines[start..end].join("\n")
}

/// The `timeout-minutes:` of the first step whose `- name:` is `step_name`.
fn step_cap(job: &str, step_name: &str) -> u32 {
    let marker = format!("- name: {step_name}");
    let at = job.find(&marker).unwrap_or_else(|| panic!("a `{step_name}` step in the job"));
    job[at..]
        .lines()
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with("- name:"))
        .find_map(|l| l.trim().strip_prefix("timeout-minutes:"))
        .unwrap_or_else(|| panic!("the `{step_name}` step carries no timeout-minutes"))
        .trim()
        .parse()
        .expect("numeric step timeout")
}

#[test]
fn rgpt_ac07_ci_job_runs_every_rgpt_test_inside_six_minutes() {
    let job = job_text();
    // The 6 minutes bound the golden RUN (a step), not a cold compile: a job-level
    // 6-minute cap cancelled the cold build before its Post step could save the
    // rust-cache, so the cache was never written (seed GPC, 10-18).
    let run_cap = step_cap(&job, "cargo test (golden paths)");
    assert_eq!(run_cap, 6, "the golden-path RUN step must be capped at 6 minutes, found {run_cap}");
    let job_cap: u32 = job
        .lines()
        .find_map(|l| l.strip_prefix("    timeout-minutes:"))
        .expect("job-level timeout-minutes")
        .trim()
        .parse()
        .expect("numeric timeout");
    assert!(job_cap > run_cap, "the job cap ({job_cap}) must exceed the run step cap ({run_cap}) so a cold compile can finish and save the cache");
    assert!(job_cap <= 20, "the job cap must stay <= 20 minutes, found {job_cap}");
    assert!(job.contains("--no-run"), "the compile must be its own step (cargo test ... --no-run) ahead of the timed run");
    assert!(job.contains("name: recipes-golden-path"), "the context name is the job name");
    assert!(!job.contains("needs:"), "the job must start with the others, not wait on them");
    assert!(job.contains("rgpt_ -- --nocapture"), "the job must run the rgpt_ tests: {job}");
    assert!(job.contains("--package xtask -- recipes --check"), "the job must check the generated files first");
    for line in job.lines().map(str::trim).filter(|l| l.starts_with("cargo ")) {
        assert!(line.contains("--locked"), "cargo line without --locked: {line}");
    }
    let required = std::fs::read_to_string(repo().join(".github/required-checks.toml")).expect("required-checks.toml");
    assert!(required.contains("\"recipes-golden-path\""), "a PR cannot merge past a red recipes-golden-path");
}

fn spec_of(name: &str) -> Value {
    let r = recipes::load_all(repo()).expect("load").into_iter().find(|r| r.name == name).expect("recipe");
    recipes::golden_spec(&r)
}

#[tokio::test]
async fn rgpt_ac07_a_renamed_fixture_route_fails_that_recipe_only() {
    let mut broken = spec_of("share-a-tool");
    let route = broken["fixture_routes"][0]["path"].as_str().expect("route").to_string();
    broken["fixture_routes"][0]["path"] = Value::String(format!("{route}-renamed"));

    let failed = tokio::spawn(async move { rgpt::run(&broken).await }).await;
    let err = failed.expect_err("the renamed route must fail share-a-tool's golden path");
    let msg = err.into_panic().downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.starts_with("share-a-tool step "), "the failure must name the broken recipe: {msg}");

    // The sibling recipes are untouched and still pass.
    for other in ["uptime-probes", "chart-in-a-minute"] {
        let trace = rgpt::run(&spec_of(other)).await;
        assert!(!trace.calls.is_empty(), "{other} ran no calls");
    }
}
