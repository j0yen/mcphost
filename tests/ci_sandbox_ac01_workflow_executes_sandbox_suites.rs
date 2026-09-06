//! PRD-mcphost-ci-sandbox-coverage
//! AC1 (P0) -- Given a push to main, When the ci workflow runs on the hosted
//! runner, Then the sandbox-ready suites execute with >0 tests run and their
//! assertions evaluated.
//!
//! Scoped decision: the "on the hosted runner" half of this AC is only
//! observable in a GitHub Actions run, and was verified there by hand
//! (runs 33955331814 and 33995726792 both report skipped=0 and 214-227
//! executed tests). What a `cargo test` CAN hold is the property that made
//! those runs vacuous before this PRD and would make them vacuous again:
//! that the workflow still routes every sandbox-ready target into a job
//! which grants the userns capability and then refuses to pass with zero
//! executed tests. That is what this test pins -- the shape a regression
//! would have to break to bring the 0.00s-suite false green back.

mod ci_sandbox_support;
use ci_sandbox_support as support;

#[test]
fn every_sandbox_ready_target_runs_in_the_capability_granting_job() {
    let sandbox_targets = support::partition(&["list", "sandbox"]);
    let sandbox_ready: Vec<&str> = sandbox_targets
        .lines()
        .filter(|t| t.starts_with("sandboxready_"))
        .collect();
    assert!(
        !sandbox_ready.is_empty(),
        "the sandbox partition contains no sandboxready_* target; either the \
         suites were deleted or scripts/ci-test-partition.sh stopped \
         recognising them:\n{sandbox_targets}"
    );

    let jobs = support::jobs(&support::workflow());
    let (_, sandbox_job) = jobs
        .iter()
        .find(|(name, _)| name == "sandbox")
        .expect("ci.yml must declare a `sandbox` job");

    assert!(
        sandbox_job.contains("kernel.apparmor_restrict_unprivileged_userns=0"),
        "the sandbox job must grant unprivileged user namespaces, or its \
         suites skip instead of running"
    );
    assert!(
        sandbox_job.contains("apt-get install -y bubblewrap"),
        "the sandbox job must install bwrap explicitly, never assume the image has it"
    );
    assert!(
        sandbox_job.contains("ci-test-partition.sh sandbox"),
        "the sandbox job must run the derived sandbox partition, not a hand-listed subset"
    );
}

#[test]
fn the_sandbox_job_refuses_to_pass_with_zero_executed_tests() {
    let jobs = support::jobs(&support::workflow());
    let (_, sandbox_job) = jobs
        .iter()
        .find(|(name, _)| name == "sandbox")
        .expect("ci.yml must declare a `sandbox` job");

    // The non-vacuous assertion: cargo test's own summary is all-green when
    // every test skipped, so "0 executed" has to be an explicit job failure.
    assert!(
        sandbox_job.contains(r#"executed=$(grep -c '^test .* \.\.\. ok$' /tmp/cargo-test.log"#),
        "the sandbox job must count executed tests from the captured log"
    );
    assert!(
        sandbox_job.contains(r#"if [ "$executed" -eq 0 ]; then"#)
            && sandbox_job.contains("did not run at all"),
        "the sandbox job must fail explicitly when zero tests executed"
    );
}
