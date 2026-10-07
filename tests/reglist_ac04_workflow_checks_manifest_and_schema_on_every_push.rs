//! PRD-mcphost-registry-listing
//! AC4 (P1) -- Given a push to any branch, When the registry workflow runs
//! on the free runner, Then it executes `--check` and schema validation
//! and fails the push on drift.
//!
//! Hand-rolled text assertions over `.github/workflows/registry.yml`
//! rather than a YAML dependency, same call `tests/ci_sandbox_support/
//! mod.rs::jobs` already made for `ci.yml`: this crate has no YAML crate,
//! and the file being parsed is one this repo owns and keeps
//! two-space-indented.

fn workflow() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".github/workflows/registry.yml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The `on:` trigger block, up to the next top-level (`permissions:`) key.
fn trigger_block(workflow: &str) -> &str {
    let start = workflow.find("\non:\n").expect("workflow must declare `on:`") + 1;
    let rest = &workflow[start..];
    let end = rest.find("\npermissions:").unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn push_trigger_has_no_branch_filter() {
    let workflow = workflow();
    let trigger = trigger_block(&workflow);
    assert!(trigger.contains("push:"), "must trigger on push: {trigger}");
    assert!(
        !trigger.contains("branches:"),
        "a branch filter would exempt some branches from the AC's \"a push to any branch\": \
         {trigger}"
    );
}

#[test]
fn check_job_runs_the_manifest_check_and_schema_validation_on_the_free_runner() {
    let workflow = workflow();
    let jobs_start = workflow.find("\njobs:\n").expect("workflow must declare jobs:");
    let jobs_block = &workflow[jobs_start..];
    let check_start = jobs_block.find("\n  check:").expect("must declare a `check` job");
    let check_block = &jobs_block[check_start..];
    let check_end = check_block[1..]
        .find("\n  publish:")
        .map(|i| i + 1)
        .unwrap_or(check_block.len());
    let check_block = &check_block[..check_end];

    assert!(
        check_block.contains("runs-on: ubuntu-latest"),
        "the check job must run on the free public-repo runner, not a paid/self-hosted one: \
         {check_block}"
    );
    assert!(
        check_block.contains("registry-manifest --check"),
        "the check job must run `mcphost registry-manifest --check` (AC2): {check_block}"
    );
    assert!(
        check_block.contains("reglist_ac03"),
        "the check job must run the schema-validation suite (AC3): {check_block}"
    );
}

#[test]
fn publish_job_only_runs_on_a_v_tag_and_depends_on_check() {
    let workflow = workflow();
    let jobs_start = workflow.find("\njobs:\n").expect("workflow must declare jobs:");
    let publish_start = workflow[jobs_start..]
        .find("\n  publish:")
        .expect("must declare a `publish` job");
    let publish_block = &workflow[jobs_start + publish_start..];

    assert!(
        publish_block.contains("needs: check"),
        "publish must depend on check succeeding first: {publish_block}"
    );
    assert!(
        publish_block.contains("refs/tags/v"),
        "publish must gate on a `v*` tag ref (AC5): {publish_block}"
    );
}
