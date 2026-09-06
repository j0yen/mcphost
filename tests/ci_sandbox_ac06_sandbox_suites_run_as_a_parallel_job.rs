//! PRD-mcphost-ci-sandbox-coverage
//! AC6 (P1) -- Given the full workflow on a clean cache, When it completes,
//! Then total added wall time for sandbox suites is <=5 minutes or the suites
//! run as a parallel job.
//!
//! The first branch was measured and missed: with the suites executing for
//! real, `cargo test --workspace` took 336s (run 33955331814, v0.13.1) and
//! 313s (run 33995726792, v0.13.3) against a 300s budget. So this AC is
//! satisfied through its second branch, and what a test can hold is that the
//! second branch is genuinely in force -- two jobs, neither waiting on the
//! other, with the sandbox suites in one of them.
//!
//! It also holds the property that makes a split safe rather than a way to
//! lose coverage: the partition must stay total and disjoint over
//! `tests/*.rs`. A split that quietly drops a target would cut wall time and
//! test nothing, which would be a worse false green than the one this PRD
//! started from.

mod ci_sandbox_support;
use ci_sandbox_support as support;

#[test]
fn the_sandbox_suites_run_as_their_own_job_in_parallel_with_the_rest() {
    let workflow = support::workflow();
    let jobs = support::jobs(&workflow);
    let names: Vec<&str> = jobs.iter().map(|(n, _)| n.as_str()).collect();

    assert!(
        jobs.len() >= 2,
        "AC6's parallel-job branch needs at least two jobs; found {names:?}"
    );

    let gate = jobs
        .iter()
        .find(|(n, _)| n == "gate")
        .expect("ci.yml must keep a `gate` job");
    let sandbox = jobs
        .iter()
        .find(|(n, _)| n == "sandbox")
        .expect("ci.yml must declare a `sandbox` job");

    // `needs:` on either job would serialise them and put the workflow's wall
    // time back at the sum this AC exists to avoid.
    for (name, body) in [gate, sandbox] {
        assert!(
            !support::declares_needs(body),
            "job `{name}` declares `needs:`; the two jobs must run concurrently \
             for AC6's parallel-job branch to hold"
        );
    }

    assert!(
        sandbox.1.contains("ci-test-partition.sh sandbox"),
        "the sandbox job must run the sandbox partition"
    );
    assert!(
        gate.1.contains("ci-test-partition.sh core"),
        "the gate job must run the core partition"
    );
}

#[test]
fn the_partition_is_total_and_disjoint_over_every_test_target() {
    // The script's own `check` is the authority (it also refuses an explicit
    // `[[test]]` target in Cargo.toml, which would break the tests/*.rs ==
    // targets assumption the split rests on).
    let checked = support::partition(&["check"]);
    assert!(
        checked.contains("ok"),
        "ci-test-partition.sh check did not report ok: {checked}"
    );

    let sandbox: Vec<String> = support::partition(&["list", "sandbox"])
        .lines()
        .map(str::to_string)
        .collect();
    let core: Vec<String> = support::partition(&["list", "core"])
        .lines()
        .map(str::to_string)
        .collect();

    let mut union: Vec<String> = sandbox.iter().chain(core.iter()).cloned().collect();
    union.sort();
    let mut deduped = union.clone();
    deduped.dedup();
    assert_eq!(union, deduped, "a test target is in both partitions");

    let mut on_disk: Vec<String> = std::fs::read_dir(support::repo_root().join("tests"))
        .expect("read tests/")
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().and_then(|x| x.to_str()) == Some("rs"))
                .then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    on_disk.sort();

    assert_eq!(
        union, on_disk,
        "the two CI jobs together must run every tests/*.rs target -- a target \
         in neither partition is silently untested in CI"
    );
}

#[test]
fn both_jobs_verify_the_partition_before_testing() {
    for (name, body) in support::jobs(&support::workflow()) {
        assert!(
            body.contains("ci-test-partition.sh check"),
            "job `{name}` must verify the partition before it trusts it"
        );
    }
}
