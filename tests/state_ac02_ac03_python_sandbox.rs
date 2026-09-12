//! PRD-mcphost-tenant-state
//! AC2 — Given a python tool whose source calls `mcphost.state.set("count",
//! mcphost.state.get("count", 0) + 1)` and returns the new value, When it
//! is called three times, Then the results are 1, 2, 3 and
//! `host.state.get(key="count")` returns 3.
//! AC3 — Given the same tool published with `network: none`, When called,
//! Then it succeeds (the sandbox has no network by default; the dedicated
//! network-isolation mechanism itself is proven by
//! `python_ac08_network_none_blocks.rs` -- this test only needs the tool to
//! actually succeed under that default).
//!
//! Requirement 3's own note: the first of the three calls is a cold call
//! (`sandbox::run`'s former one-shot path, now `run_cold_interactive`); the
//! second and third land on the warm pool
//! (`kinds::python::PythonKind::try_warm`) once the first call promotes it.
//! Both paths go through `sandbox::PersistentSandbox::call`'s interactive
//! protocol, so this test exercises both.

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcphost_state_persists_a_counter_across_three_calls() {
    // This test runs a real python-kind tool via the sandbox, which needs
    // unprivileged user namespaces -- same skip-in-CI, fail-loudly-elsewhere
    // convention every other sandbox-dependent test in this crate uses (see
    // `tests/python_ac01_no_deps_cpu_memory.rs`).
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "State AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import mcphost\ndef main(args):\n    n = mcphost.state.get(\"count\", 0) + 1\n    mcphost.state.set(\"count\", n)\n    return {\"n\": n}\n",
        // AC3: no `network` key at all -- the python kind's own default is
        // "none" (see `kinds::python::parse_spec`'s `"network"` field
        // default), so this is exactly the "published with network: none"
        // scenario AC3 describes.
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "counter", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let qualified = format!("{ns}.counter");

    // Call 1: cold (`run_cold_interactive`).
    let first = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("first call must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&first)["n"], json!(1));

    // Calls 2 and 3: warm (`PersistentSandbox::call`, promoted after call 1).
    let second = client
        .tools_call(&qualified, json!({}))
        .await
        .expect("second call must succeed");
    assert_eq!(extract_structured(&second)["n"], json!(2));

    let third = client
        .tools_call(&qualified, json!({}))
        .await
        .expect("third call must succeed");
    assert_eq!(extract_structured(&third)["n"], json!(3));

    // The agent's own view of the same key (`host.state.get`) agrees.
    let got = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "count"}))
            .await
            .expect("host.state.get"),
    );
    assert_eq!(got["value"], json!(3));
}
