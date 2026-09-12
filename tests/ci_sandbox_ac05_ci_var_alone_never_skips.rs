//! PRD-mcphost-ci-sandbox-coverage
//! AC5 (P0) -- Given the CI env var set but capability present, When the
//! suite runs, Then tests execute -- the variable alone never causes a skip.
//!
//! This is the AC the whole PRD turns on: before it, the guard short-circuited
//! on `$CI` and the hosted runner's suites finished in 0.00s. The child here
//! is the adversarial case -- `CI=true`, exactly as GitHub Actions sets it,
//! on a box that CAN create user namespaces -- and it must still run.

use crate::ci_sandbox_support;
use ci_sandbox_support as support;

use mcphost::sandbox::{UsernsDecision, decide_userns};

#[test]
fn ci_set_with_capability_present_still_runs_the_suite() {
    if std::env::var_os(support::CHILD_ROLE_ENV).is_some() {
        support::child_report_guard();
    }

    // The decision never consults `ci` while `capable` holds: both CI values
    // give the same answer.
    assert_eq!(decide_userns(true, true), UsernsDecision::Run);
    assert_eq!(decide_userns(true, false), UsernsDecision::Run);

    // "but capability present" is this AC's premise. Naming the probe here
    // also routes this target into the sandbox partition (see
    // `scripts/ci-test-partition.sh`), which is the CI job that grants it.
    assert!(
        mcphost::sandbox::supports_user_namespaces(),
        "AC5 requires a box that can create user namespaces; in CI that is the \
         `sandbox` job's sysctl grant, locally see README's user-namespace section"
    );

    // PRD-mcphost-test-suite-consolidation: module-qualified name, see the
    // comment in ci_sandbox_ac02_capable_env_never_skips.rs.
    let out = support::run_guard_child(
        &format!(
            "{}::ci_set_with_capability_present_still_runs_the_suite",
            support::strip_crate_root(module_path!())
        ),
        true,
        true,
    );
    let stdout = support::stdout_of(&out);
    assert!(
        out.status.success(),
        "guard must not fail under CI=true on a capable box.\nstdout: {stdout}\nstderr: {}",
        support::stderr_of(&out)
    );
    assert!(
        stdout.contains(&format!("{}false", support::GUARD_RESULT_PREFIX)),
        "CI=true alone must never cause a skip on a capable box; got:\n{stdout}"
    );
    assert!(
        !stdout.contains(mcphost::sandbox::USERNS_SKIP_MARKER),
        "no capability-skip line may be emitted when the capability is present; got:\n{stdout}"
    );
}
