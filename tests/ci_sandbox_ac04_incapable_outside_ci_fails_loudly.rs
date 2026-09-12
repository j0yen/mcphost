//! PRD-mcphost-ci-sandbox-coverage
//! AC4 (P0) -- Given a dev box without userns and CI unset, When the suite
//! runs, Then the guard fails loudly (no silent skip), unchanged from the
//! 2026-09-03 contract.
//!
//! This is the one cell of the truth table that cannot be observed by calling
//! the guard in-process: it panics. The child re-invocation makes it
//! observable for real -- no `unshare` on `$PATH`, `$CI` removed -- and the
//! child prints `guard-returned: ...` if and only if the guard RETURNED. So
//! "no silent skip" is asserted as the absence of that line plus a non-zero
//! exit, not as a source-text read.

use crate::ci_sandbox_support;
use ci_sandbox_support as support;

use mcphost::sandbox::{UsernsDecision, decide_userns};

#[test]
fn incapable_outside_ci_panics_instead_of_skipping() {
    if std::env::var_os(support::CHILD_ROLE_ENV).is_some() {
        support::child_report_guard();
    }

    assert_eq!(
        decide_userns(false, false),
        UsernsDecision::FailLoudly,
        "an incapable box outside CI must fail loudly, never skip"
    );

    // PRD-mcphost-test-suite-consolidation: module-qualified name, see the
    // comment in ci_sandbox_ac02_capable_env_never_skips.rs.
    let out = support::run_guard_child(
        &format!(
            "{}::incapable_outside_ci_panics_instead_of_skipping",
            support::strip_crate_root(module_path!())
        ),
        false,
        false,
    );
    let stdout = support::stdout_of(&out);
    let stderr = support::stderr_of(&out);

    assert!(
        !stdout.contains(support::GUARD_RESULT_PREFIX),
        "the guard returned instead of panicking on an incapable non-CI box \
         -- that is the silent skip AC4 forbids.\nstdout: {stdout}"
    );
    assert!(
        !out.status.success(),
        "an incapable non-CI box must fail the test binary.\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("refusing to silently skip outside CI"),
        "the panic must say why it refused to skip; got stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("apparmor_restrict_unprivileged_userns")
            || stderr.contains("unprivileged_userns_clone"),
        "the panic must name the fix, not just the symptom; got stderr:\n{stderr}"
    );
}
