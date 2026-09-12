//! PRD-mcphost-ci-sandbox-coverage
//! AC3 (P0) -- Given a runner where userns creation genuinely fails, When the
//! suite runs in CI, Then affected tests skip with the capability reason in
//! the log AND the non-vacuous assertion fails the job.
//!
//! Two halves, both checkable here. The skip-with-reason half runs the real
//! guard in a child with no `unshare` on `$PATH` and `CI=true`, and asserts
//! the child's log carries the capability reason. The fail-the-job half is a
//! property of `.github/workflows/ci.yml`: it must grep for the same literal
//! and exit non-zero on a hit.
//!
//! The literal is the whole point. The grep in YAML and the `println!` in
//! Rust are one contract spanning two files and two languages, and the only
//! thing joining them is `sandbox::USERNS_SKIP_MARKER`. Reword the Rust side
//! alone and the grep silently matches nothing -- CI goes green on a run
//! where every sandbox test skipped, which is precisely the failure this PRD
//! was written to remove. So this test asserts the workflow contains the
//! constant's *value*, and that every skip line in `tests/` is built from it.

use crate::ci_sandbox_support;
use ci_sandbox_support as support;

use mcphost::sandbox::{USERNS_SKIP_MARKER, UsernsDecision, decide_userns};

#[test]
fn incapable_in_ci_skips_with_the_capability_reason() {
    if std::env::var_os(support::CHILD_ROLE_ENV).is_some() {
        support::child_report_guard();
    }

    assert_eq!(
        decide_userns(false, true),
        UsernsDecision::SkipInCi,
        "an incapable box inside CI must skip cleanly"
    );

    // PRD-mcphost-test-suite-consolidation: module-qualified name, see the
    // comment in ci_sandbox_ac02_capable_env_never_skips.rs.
    let out = support::run_guard_child(
        &format!(
            "{}::incapable_in_ci_skips_with_the_capability_reason",
            support::strip_crate_root(module_path!())
        ),
        false,
        true,
    );
    let stdout = support::stdout_of(&out);
    assert!(
        out.status.success(),
        "an incapable CI box must skip, not fail.\nstdout: {stdout}\nstderr: {}",
        support::stderr_of(&out)
    );
    assert!(
        stdout.contains(&format!("{}true", support::GUARD_RESULT_PREFIX)),
        "guard must return true (skip) when incapable inside CI; got:\n{stdout}"
    );
    assert!(
        stdout.contains(USERNS_SKIP_MARKER),
        "the skip must name the capability reason in the log; got:\n{stdout}"
    );
}

#[test]
fn the_workflow_greps_for_the_marker_the_rust_side_actually_prints() {
    let workflow = support::workflow();
    assert!(
        workflow.contains(&format!(r#"grep -c "{USERNS_SKIP_MARKER}""#)),
        "ci.yml must grep for sandbox::USERNS_SKIP_MARKER's exact value \
         ({USERNS_SKIP_MARKER:?}); a reworded marker with a stale grep is a \
         silent false green"
    );

    // Only jobs that actually invoke `cargo test` can skip a test for
    // capability reasons; a job like `sandbox-required` that merely gates on
    // another job's aggregate matrix result (see AC6's shard split) has no
    // per-test skip to detect and is exempt from this invariant.
    for (name, body) in support::jobs(&workflow) {
        if !body.contains("cargo test") {
            continue;
        }
        assert!(
            body.contains(r#"if [ "$skipped" -gt 0 ]; then"#) && body.contains("exit 1"),
            "job `{name}` must fail when any test skipped for capability reasons"
        );
    }
}

#[test]
fn no_rust_source_hardcodes_the_marker_text_instead_of_the_constant() {
    // The single legitimate occurrence: the constant's own definition.
    let definition = format!(r#"pub const USERNS_SKIP_MARKER: &str = "{USERNS_SKIP_MARKER}";"#);

    let mut offenders: Vec<String> = Vec::new();
    let mut stack = vec![support::repo_root().join("src"), support::repo_root().join("tests")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                let trimmed = line.trim();
                if !trimmed.contains(USERNS_SKIP_MARKER)
                    || trimmed == definition
                    || trimmed.starts_with("//")
                {
                    continue;
                }
                offenders.push(format!("{}:{}: {trimmed}", p.display(), i + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these lines hardcode the capability-skip text instead of using \
         sandbox::USERNS_SKIP_MARKER, so ci.yml's grep can silently stop \
         matching them after a reword:\n{}",
        offenders.join("\n")
    );
}
