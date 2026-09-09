//! AC8 (P1, PRD-mcphost-call-limits-honest) — Given a tenant that received
//! 20 `capacity` refusals in a minute, When `host.usage(window="1m")` is
//! read, Then `capacity_refusals` is 20.
//!
//! Uses `python_kind_registry_with_concurrency(_, 1)` (host-wide cap 1,
//! well under the free plan's own per-tenant cap of 4) so every refusal in
//! this test is scope `"host"`, not `"tenant"` -- `Db::usage`'s
//! `capacity_refusals` count is scope-agnostic (it counts
//! `error_class = "capacity"` regardless of which admission gate produced
//! it), so either scope proves the same counting property; this test
//! only needs a cheap, reliable way to manufacture exactly 20 refusals; a
//! host-wide cap of 1 does that with the smallest possible burst.

mod common;
use common::{TestServer, poll_until_ready, python_kind_registry_with_concurrency, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn twenty_capacity_refusals_are_counted_in_host_usage() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server =
        TestServer::start_with_kinds(python_kind_registry_with_concurrency(&envs_dir.0, 1)).await;
    let (ns, key) = signup(&server.base_url, "AC8 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(2)\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.slow");
    let _ = poll_until_ready(&client, &qualified, json!({}), Duration::from_secs(20)).await;

    // One long call holds the sole host-wide slot; 20 more fired
    // concurrently must all be refused with `capacity` while it runs.
    let holder = {
        let c = common::McpClient::with_bearer(&server.base_url, &key);
        let q = qualified.clone();
        tokio::spawn(async move { c.tools_call(&q, json!({})).await })
    };
    // Let the holder actually acquire the slot before firing refusals at it.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut handles = Vec::with_capacity(20);
    for _ in 0..20 {
        let c = common::McpClient::with_bearer(&server.base_url, &key);
        let q = qualified.clone();
        handles.push(tokio::spawn(async move { c.tools_call(&q, json!({})).await }));
    }
    let mut capacity_count = 0;
    for handle in handles {
        match handle.await.expect("task must not panic") {
            Err(e) if e.error_code.as_deref() == Some("capacity") => capacity_count += 1,
            other => panic!("expected capacity, got {other:?}"),
        }
    }
    assert_eq!(capacity_count, 20);
    holder
        .await
        .expect("task must not panic")
        .expect("the slot holder's own call must succeed");

    let usage = client
        .tools_call("host.usage", json!({"window": "1m"}))
        .await
        .expect("host.usage ok");
    let structured = common::extract_structured(&usage);
    assert_eq!(structured["capacity_refusals"], json!(20));
}
