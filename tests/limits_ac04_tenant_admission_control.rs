//! AC4 (PRD-mcphost-call-limits-honest) — Given plan `free` with
//! `concurrent_calls_per_tenant: 4`, When tenant A issues 50 concurrent
//! calls and tenant B one call, Then B's call succeeds and at least 40 of
//! A's refusals are `capacity` with `scope: "tenant"` and a positive
//! `retry_after_ms`.
//!
//! Uses the default plan catalog (`free` = 4, the same number this AC
//! names) and the default host-wide concurrency ceiling (20) -- tenant A's
//! own admission semaphore (checked *before* the host-wide one, see
//! `kinds::python::PythonKind::call`) is what actually binds here; the
//! host-wide cap is never contended by this test (at most 4 of A's calls
//! and 1 of B's are ever in flight together, well under 20).

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn tenant_burst_is_capped_at_its_own_plan_limit_while_another_tenant_still_runs() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns_a, key_a) = signup(&server.base_url, "AC4 Tenant A").await;
    let (ns_b, key_b) = signup(&server.base_url, "AC4 Tenant B").await;
    let client_a = common::McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = common::McpClient::with_bearer(&server.base_url, &key_b);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(3)\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
    });
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow", "kind": "python", "spec": spec.clone()}),
        )
        .await
        .expect("publish ok (A)");
    client_b
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok (B)");

    let qualified_a = format!("{ns_a}.slow");
    let qualified_b = format!("{ns_b}.slow");

    // Warm both environments (a fast call each) before the timed burst, so
    // the burst measures admission control, not env-build races.
    let _ = poll_until_ready(&client_a, &qualified_a, json!({}), Duration::from_secs(20))
        .await;
    let _ = poll_until_ready(&client_b, &qualified_b, json!({}), Duration::from_secs(20))
        .await;

    // Fire tenant A's 50-way burst and tenant B's single call together.
    let mut handles = Vec::with_capacity(51);
    for _ in 0..50 {
        let c = common::McpClient::with_bearer(&server.base_url, &key_a);
        let q = qualified_a.clone();
        handles.push(tokio::spawn(async move {
            ("A", c.tools_call(&q, json!({})).await)
        }));
    }
    let b_handle = {
        let c = common::McpClient::with_bearer(&server.base_url, &key_b);
        let q = qualified_b.clone();
        tokio::spawn(async move { ("B", c.tools_call(&q, json!({})).await) })
    };

    let mut ok_a = 0;
    let mut capacity_a = 0;
    let mut capacity_a_scopes_ok = true;
    let mut capacity_a_retry_positive = true;
    for handle in handles {
        let (who, result) = handle.await.expect("task must not panic");
        assert_eq!(who, "A");
        match result {
            Ok(_) => ok_a += 1,
            Err(e) if e.error_code.as_deref() == Some("capacity") => {
                capacity_a += 1;
                if e.data.get("scope").and_then(serde_json::Value::as_str) != Some("tenant") {
                    capacity_a_scopes_ok = false;
                }
                let retry = e
                    .data
                    .get("retry_after_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                if retry == 0 {
                    capacity_a_retry_positive = false;
                }
            }
            Err(e) => panic!("unexpected error for tenant A: {} {}", e.code, e.message),
        }
    }
    let (who_b, result_b) = b_handle.await.expect("task must not panic");
    assert_eq!(who_b, "B");
    result_b.expect("tenant B's single call must succeed while A is bursting");

    assert!(
        capacity_a >= 40,
        "expected at least 40 of A's 50 calls refused with capacity, got {capacity_a} \
         (ok={ok_a})"
    );
    assert!(
        capacity_a_scopes_ok,
        "every capacity refusal for tenant A's own burst must carry scope: \"tenant\""
    );
    assert!(
        capacity_a_retry_positive,
        "every capacity refusal must carry a positive retry_after_ms"
    );
    assert!(
        ok_a >= 1 && ok_a <= 4,
        "tenant A must never exceed its own plan cap of 4 concurrent calls, got {ok_a}"
    );
}
