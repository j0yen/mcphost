//! PRD-mcphost-sandbox-egress-allowlist
//! AC10 (P0) — Given the existing test suites for python and wasm kinds,
//! When run after this change, Then no fixture that previously relied on
//! Free network remains green by accident.
//!
//! This is the regression proof for the specific hole this PRD closes:
//! `tests/support/uptime_probes.rs`'s own `probe` tool (used by six
//! `mcphost_uptime_probes_ac0{2,3,4,5,7,9}_*` tests) has always declared
//! `network: "public"`, and used to publish fine for ANY tenant, Free
//! included, because only the `"egress"` spelling was plan-gated before
//! this PRD (src/control.rs's old check). Those six fixtures are updated
//! (see the receipt in this PRD's own build notes) to a `pro` tenant plus
//! `$MCPHOST_EGRESS_PROXY`, via the new `uptime_probes::grant_egress`
//! helper, so they keep exercising real network instead of downgrading to
//! `network: "none"` and losing that coverage. This test proves the
//! specific shape that used to slip through -- the exact fixture source,
//! published by a plain Free tenant -- is refused now.
//!
//! The `wasm` kind has no `network` spec field at all (its sandbox never
//! grants network regardless of plan -- see `src/kinds/wasm.rs`), so no
//! wasm fixture relied on Free network to begin with; nothing there needed
//! updating.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

/// The exact source `tests/support/uptime_probes.rs::probe_source()` reads
/// for the six fixtures this AC is about -- read directly (rather than
/// pulling in that whole support module, which carries unrelated helpers
/// this one test doesn't call) so this file is self-contained regardless
/// of which `tests/suite_*.rs` binary it ends up bucketed into.
fn probe_source() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/uptime-probes/tools/probe.py"),
    )
    .expect("read examples/uptime-probes/tools/probe.py")
}

#[tokio::test]
async fn the_uptime_probes_fixture_spec_is_refused_for_a_free_tenant() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC10 Free Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({"source": probe_source(), "network": "public"});
    let err = client
        .tools_call("host.tool_publish", json!({"name": "probe", "kind": "python", "spec": spec}))
        .await
        .expect_err("the real uptime-probes fixture spec must be refused for a free tenant");
    assert_eq!(err.error_code.as_deref(), Some("plan_required"));

    let tools = extract_structured(
        &client.tools_call("host.tool_list", json!({})).await.expect("tool_list"),
    );
    assert_eq!(
        tools["tools"].as_array().expect("tools array").len(),
        0,
        "no tool row must exist: {tools:?}"
    );
}
