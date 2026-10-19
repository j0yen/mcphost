//! PRD-mcphost-generated-dispatch-table-stack-budget
//! AC3 — Given a forwarded `host.whoami` on a debug build, When
//! `scripts/fwd-stack-measure.sh --frame dispatch_tenant_tool_inner` measures the
//! dispatcher's poll frame after every arm body moved into its `arm_*` function
//! returning `Pin<Box<dyn Future>>`, Then the frame is <= 65,536 B (was 454 K)
//! and `dispatch_tenant_tool_inner` contains no `match name` arm bodies, only the
//! pre-checks, the lookup, the call, and the `unknown_host_tool` fallthrough.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;
use std::process::Command;

/// The poll-frame ceiling, in bytes (was 454 K).
const FRAME_CEILING: usize = 65_536;

/// The text of `async fn dispatch_tenant_tool_inner` in `src/handler.rs`.
fn dispatcher_source() -> String {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/handler.rs")).expect("handler.rs");
    let start = src.find("    async fn dispatch_tenant_tool_inner(").expect("dispatch_tenant_tool_inner");
    let rest = &src[start..];
    let end = rest.find("\n    }\n").expect("end of dispatch_tenant_tool_inner");
    rest[..end].to_string()
}

#[test]
fn fwdtab_ac03_dispatcher_poll_frame_is_at_most_64_kib() {
    // The frame is read from the debug binary's disassembly: a release test run has
    // no symbols to read (`strip = true`), and AC3 is a debug-build claim.
    if !cfg!(debug_assertions) {
        return;
    }
    let root = env!("CARGO_MANIFEST_DIR");
    let out = Command::new("bash")
        .current_dir(root)
        .args(["scripts/fwd-stack-measure.sh", "--frame", "dispatch_tenant_tool_inner"])
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .output()
        .expect("run fwd-stack-measure.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "fwd-stack-measure.sh --frame failed: {stdout} {}", String::from_utf8_lossy(&out.stderr));
    let bytes: usize = stdout
        .trim()
        .strip_prefix("frame dispatch_tenant_tool_inner ")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("unexpected measure output: {stdout}"));
    println!("dispatch_tenant_tool_inner poll frame: {bytes} B (ceiling {FRAME_CEILING})");
    assert!(bytes <= FRAME_CEILING, "dispatch_tenant_tool_inner poll frame is {bytes} B > {FRAME_CEILING} B");
}

#[test]
fn fwdtab_ac03_dispatcher_holds_no_arm_bodies() {
    let body = dispatcher_source();
    assert!(!body.contains("match name"), "no `match name` arm bodies left in the dispatcher");
    assert!(!body.contains("=> crate::"), "no arm body left in the dispatcher");
    for needed in [
        "refuse_fields_on_unprojected",
        "validate_host_args",
        "note_handler_run",
        "dispatch_table::lookup",
        "unknown_host_tool",
    ] {
        assert!(body.contains(needed), "the dispatcher keeps `{needed}`");
    }
}

#[tokio::test]
async fn fwdtab_ac03_forwarded_whoami_still_answers() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let call = client
        .tools_call("host.tool_call", json!({"name": "host.whoami", "args": {}}))
        .await
        .expect("forwarded host.whoami");
    assert_eq!(extract_structured(&call)["forwarded_to"], "host.whoami", "{call}");
}
