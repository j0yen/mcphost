//! PRD-mcphost-generated-dispatch-table-stack-budget
//! AC4 — Given the same forwarded call, When the inline arms of
//! `call_tool_inner`'s `match (auth, name)` for `host.quickstart`,
//! `host.help.search`, `host.help.get`, `host.errors`, `host.feedback.*`,
//! `host.call_trace`, `host.doctor` have moved into boxed helpers, Then
//! `call_tool_inner`'s poll frame is <= 163,840 B (was 260-310 K) and
//! `tests/verbfwd_ac0[1-7]*` plus the quickstart, help, errors and feedback
//! suites pass unchanged (those suites are the unchanged files themselves).

use std::process::Command;

/// The poll-frame ceiling, in bytes (was 262,152 B).
const FRAME_CEILING: usize = 163_840;

/// The text of `call_tool_inner` in `src/handler.rs`.
fn call_tool_inner_source() -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/handler.rs")).expect("handler.rs");
    let start = src.find("    pub(crate) async fn call_tool_inner(").expect("call_tool_inner");
    let rest = &src[start..];
    let end = rest.find("\n    }\n").expect("end of call_tool_inner");
    rest[..end].to_string()
}

#[test]
fn fwdtab_ac04_call_tool_inner_poll_frame_is_at_most_160_kib() {
    // Read from the debug binary's disassembly; a release test run has no symbols.
    if !cfg!(debug_assertions) {
        return;
    }
    let out = Command::new("bash")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["scripts/fwd-stack-measure.sh", "--frame", "call_tool_inner"])
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .output()
        .expect("run fwd-stack-measure.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "fwd-stack-measure.sh --frame failed: {stdout} {}", String::from_utf8_lossy(&out.stderr));
    let bytes: usize = stdout
        .trim()
        .strip_prefix("frame call_tool_inner ")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("unexpected measure output: {stdout}"));
    println!("call_tool_inner poll frame: {bytes} B (ceiling {FRAME_CEILING})");
    assert!(bytes <= FRAME_CEILING, "call_tool_inner poll frame is {bytes} B > {FRAME_CEILING} B");
}

#[test]
fn fwdtab_ac04_inline_arms_live_in_boxed_helpers() {
    let body = call_tool_inner_source();
    for (helper, moved_body) in [
        ("quickstart_tenant_boxed", "control::quickstart(&self.state, Some(tenant)"),
        ("quickstart_anonymous", "control::quickstart(&self.state, None"),
        ("help_search_boxed", "help_search::call_observed"),
        ("help_get_boxed", "doc_anchors::help_get"),
        ("errors_boxed", "error_registry::call"),
        ("feedback_boxed", "feedback::send("),
        ("call_trace_admin_boxed", "self.call_trace("),
        ("doctor_admin_boxed", "self.doctor("),
    ] {
        assert!(body.contains(helper), "call_tool_inner calls `{helper}`");
        assert!(!body.contains(moved_body), "call_tool_inner no longer holds the `{moved_body}` arm body");
    }
}
