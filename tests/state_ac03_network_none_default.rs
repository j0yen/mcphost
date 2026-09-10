//! PRD-mcphost-tenant-state
//! AC3 — Given the same tool published with `network: none`, When called,
//! Then it succeeds and the sandbox has no network (the existing
//! network-isolation test still passes for it).
//!
//! This is a dedicated, discoverable-under-the-`state_` prefix pairing for
//! AC3 (see `tests/state_ac02_ac03_python_sandbox.rs`'s own doc comment:
//! that file's single test already exercises AC2 and AC3 together, but the
//! AC-derivation tool only pairs the first `ac<N>` token immediately after
//! a declared prefix, so AC3 needs its own file to pair). This test is
//! deliberately thin -- one cold call, no counter, no warm-pool exercise
//! (that is AC2's job) -- and asserts specifically the claim AC3 makes: a
//! `mcphost.state`-using tool published with no `network` key at all (the
//! python kind's own default is `"none"`, per `kinds::python::parse_spec`)
//! still succeeds. The dedicated network-isolation *mechanism* itself
//! (that the sandbox truly has no route) is proven by
//! `python_ac08_network_none_blocks.rs`; this test only needs the tool to
//! actually succeed under that default, exactly as AC3 says.

mod common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn state_tool_published_with_no_network_key_succeeds() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "State AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import mcphost\ndef main(args):\n    mcphost.state.set(\"seen\", True)\n    return {\"ok\": mcphost.state.get(\"seen\", False)}\n",
        // No "network" key at all -- AC3's exact scenario: the python
        // kind's own default is "none".
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "no_network_default", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.no_network_default");
    let result = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("call under the default network:none must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&result)["ok"], json!(true));
}
