//! PRD-mcphost-sandbox-ready
//! AC7 -- Given the conformance suite, When it runs against an injected
//! unready status, Then `conformance::check_rejection_shape` (extended:
//! `conformance::check_sandbox_unavailable_shape`) asserts the
//! `sandbox_unavailable` shape for `host.tool_publish`, `host.tool_test`,
//! and `host.tool_run`, and the suite is green.

use mcphost::kinds::conformance::check_sandbox_unavailable_shape;
use mcphost::sandbox::{IsolationMechanism, SandboxStatus};

/// The shape `AppError::sandbox_unavailable` produces is identical
/// regardless of which of the three RPCs (`host.tool_publish`,
/// `host.tool_test`, `host.tool_run`) triggered it -- all three call the
/// exact same constructor (see `control.rs`/`handler.rs`'s three call
/// sites) -- so one shape check covers all three; the end-to-end wiring
/// proof that each RPC actually reaches it lives in
/// `tests/sandboxready_ac3_ac4_publish_rejection.rs`.
#[test]
fn injected_unready_status_produces_the_full_sandbox_unavailable_shape() {
    let status = SandboxStatus {
        ready: false,
        mechanism: IsolationMechanism::Bwrap,
        detail: "userns_denied: bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted"
            .to_string(),
        checked_at: "2026-09-04T18:44:59Z".to_string(),
    };
    check_sandbox_unavailable_shape(&status).expect("shape must be complete for every RPC surface");
}

/// The check itself must fail loudly on a misuse (a ready status has no
/// `sandbox_unavailable` rejection to check the shape of) rather than
/// silently reporting success.
#[test]
fn a_ready_status_is_a_conformance_failure_not_a_silent_pass() {
    let status = SandboxStatus::ready(IsolationMechanism::Bwrap);
    assert!(check_sandbox_unavailable_shape(&status).is_err());
}
