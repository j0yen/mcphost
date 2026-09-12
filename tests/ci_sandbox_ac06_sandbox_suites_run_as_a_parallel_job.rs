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

use crate::ci_sandbox_support;
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
    // The script's own `check` is the authority (it also verifies
    // gen-test-suites.sh's suites aren't drifted/incomplete, which is what
    // the tests/*.rs == suite-membership assumption below rests on --
    // PRD-mcphost-test-suite-consolidation).
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

    // The two lists name SUITE binaries now (`suite_core_NN`/
    // `suite_sandbox_NN`), not individual `tests/*.rs` files -- those are
    // trivially disjoint by construction (a suite is only ever core or
    // sandbox). What AC6 actually needs held is that every real member FILE
    // a suite `#[path]`-includes is covered by exactly one of the two
    // partitions, so that's what's asserted below, one level down from the
    // suite name.
    let mut union: Vec<String> = sandbox.iter().chain(core.iter()).cloned().collect();
    union.sort();
    let mut deduped = union.clone();
    deduped.dedup();
    assert_eq!(union, deduped, "a suite is listed in both partitions");

    let repo_root = support::repo_root();
    let member_files_of = |suites: &[String]| -> Vec<String> {
        let mut out = Vec::new();
        for suite in suites {
            let path = repo_root.join("tests").join(format!("{suite}.rs"));
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for line in content.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix(r#"#[path = ""#) {
                    if let Some(name) = rest.strip_suffix(r#".rs"]"#) {
                        out.push(name.to_string());
                    }
                }
            }
        }
        out
    };

    let mut sandbox_members = member_files_of(&sandbox);
    let mut core_members = member_files_of(&core);
    sandbox_members.sort();
    core_members.sort();

    let mut member_union: Vec<String> = sandbox_members.iter().chain(core_members.iter()).cloned().collect();
    member_union.sort();
    let mut member_deduped = member_union.clone();
    member_deduped.dedup();
    assert_eq!(
        member_union, member_deduped,
        "a tests/*.rs file is included by both a core and a sandbox suite"
    );

    // Exclude exactly the generator's OWN driver files (the suite binaries
    // named in `sandbox`/`core` above, e.g. `suite_core_01`), not every file
    // that happens to start with `suite_` -- a `suite_ac*` selftest fixture
    // (this PRD's own AC9 cases) is a real source file the generator
    // includes like any other, and a prefix-based exclusion here would
    // silently drop it from `on_disk`, making it look uncovered.
    let mut on_disk: Vec<String> = std::fs::read_dir(repo_root.join("tests"))
        .expect("read tests/")
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            if p.extension().and_then(|x| x.to_str()) != Some("rs")
                || sandbox.contains(&stem)
                || core.contains(&stem)
            {
                return None;
            }
            Some(stem)
        })
        .collect();
    on_disk.sort();

    assert_eq!(
        member_union, on_disk,
        "the two CI jobs together must run every tests/*.rs file -- a file \
         included by neither partition's suites is silently untested in CI"
    );
}

#[test]
fn both_jobs_verify_the_partition_before_testing() {
    // `sandbox-required` runs no tests of its own -- it only gates on the
    // `sandbox` matrix's aggregate result (AC6's shard split) -- so it has no
    // partition to trust and is exempt from this invariant.
    for (name, body) in support::jobs(&support::workflow()) {
        if !body.contains("cargo test") {
            continue;
        }
        assert!(
            body.contains("ci-test-partition.sh check"),
            "job `{name}` must verify the partition before it trusts it"
        );
    }
}
