//! PRD-mcphost-ci-sandbox-coverage
//! AC2 (P0) -- Given the capability probe on a userns-capable environment,
//! When tests start, Then no suite skips for capability reasons.
//!
//! Runs the real `require_user_namespaces_or_ci_skip()` in a child process
//! that inherits a working `$PATH` (so the probe finds `unshare` and
//! succeeds) with `$CI` unset -- see `ci_sandbox_support` for why the child
//! process, and not `set_var`, is how this suite varies the environment.

mod ci_sandbox_support;
use ci_sandbox_support as support;

use mcphost::sandbox::{UsernsDecision, decide_userns};

#[test]
fn capable_box_runs_the_suite_instead_of_skipping() {
    if std::env::var_os(support::CHILD_ROLE_ENV).is_some() {
        support::child_report_guard();
    }

    assert_eq!(
        decide_userns(true, false),
        UsernsDecision::Run,
        "a capable box outside CI must run, never skip"
    );

    // "on a userns-capable environment" is this AC's premise, so state it as
    // a precondition rather than letting the child fail with a confusing
    // mismatch. It also puts this target in the sandbox partition:
    // `scripts/ci-test-partition.sh` classifies on exactly this symbol, so
    // naming the probe here is what routes the test to the CI job that grants
    // the capability.
    assert!(
        mcphost::sandbox::supports_user_namespaces(),
        "AC2 requires a box that can create user namespaces; in CI that is the \
         `sandbox` job's sysctl grant, locally see README's user-namespace section"
    );

    let out = support::run_guard_child("capable_box_runs_the_suite_instead_of_skipping", true, false);
    let stdout = support::stdout_of(&out);
    assert!(
        out.status.success(),
        "the guard must not fail on a userns-capable box.\nstdout: {stdout}\nstderr: {}",
        support::stderr_of(&out)
    );
    assert!(
        stdout.contains(&format!("{}false", support::GUARD_RESULT_PREFIX)),
        "guard must return false (run for real) on a capable box; got:\n{stdout}"
    );
    assert!(
        !stdout.contains(mcphost::sandbox::USERNS_SKIP_MARKER),
        "a capable box must not emit the capability-skip line at all; got:\n{stdout}"
    );
}
