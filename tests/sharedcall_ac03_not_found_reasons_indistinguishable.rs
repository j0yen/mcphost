//! PRD-mcphost-shared-tool-call-path
//! AC3 — Given three qualified names (nonexistent namespace, existing
//! namespace with nonexistent tool, existing unshared tool), When B calls
//! each 200 times, Then the response bodies are byte-identical and the p95
//! latencies are within 10 ms of each other.
//!
//! All three cases probe the exact same literal qualified name
//! (`acme.lookup`) -- the *backing database state* changes between phases
//! (no such tenant, then the tenant with no such tool, then the tenant's
//! tool left private), not the string the caller sends, since the
//! non-leaking contract is about withholding server-side state, not about
//! echoing back something the caller already typed itself. The "acme"
//! tenant is seeded directly via `Db::create_tenant` (bypassing `signup`,
//! same convention as `common::bare_tenant`) so its namespace is the exact
//! literal this test controls, not `signup`'s own randomly generated one.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::auth::{generate_key, hash_key};
use serde_json::{Value, json};
use std::time::Instant;

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    if sorted_ms.is_empty() {
        return 0.0;
    }
    let idx = ((p * (sorted_ms.len() as f64 - 1.0)).round() as usize).min(sorted_ms.len() - 1);
    sorted_ms[idx]
}

/// Calls `host.tool_call {name: qualified}` as `client`, `n` times,
/// asserting every one of the `n` calls fails with the exact same
/// error shape (never a mix, never an unexpected success) -- returns that
/// one shared shape plus every call's latency in ms.
async fn probe(client: &McpClient, qualified: &str, n: usize) -> (Value, Vec<f64>) {
    let mut durations = Vec::with_capacity(n);
    let mut shape: Option<Value> = None;
    for _ in 0..n {
        let start = Instant::now();
        let result = client.tools_call("host.tool_call", json!({"name": qualified})).await;
        durations.push(start.elapsed().as_secs_f64() * 1000.0);
        let err = result.expect_err("a not-found qualified name must never succeed");
        let this_shape = json!({
            "code": err.code,
            "error_code": err.error_code,
            "message": err.message,
            "data": err.data,
        });
        match &shape {
            None => shape = Some(this_shape),
            Some(expected) => assert_eq!(
                &this_shape, expected,
                "response shape drifted across repeated calls of the same qualified name"
            ),
        }
    }
    (shape.unwrap(), durations)
}

fn p95_ms(mut durations: Vec<f64>) -> f64 {
    durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    percentile(&durations, 0.95)
}

#[tokio::test]
async fn three_not_found_reasons_are_byte_identical_and_equally_fast() {
    let server = TestServer::start().await;
    let (_ns_b, key_b) = signup(&server.base_url, "Tenant B").await;
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);

    let qualified = "acme.lookup";

    // Case 1: nonexistent namespace -- no tenant "acme" exists yet.
    let (shape_nonexistent_ns, durations_nonexistent_ns) = probe(&client_b, qualified, 200).await;

    // Case 2: existing namespace, nonexistent tool.
    let owner_key = generate_key();
    server
        .state
        .db
        .create_tenant("Acme Corp".to_string(), "acme".to_string(), hash_key(&owner_key), None)
        .await
        .expect("create acme tenant directly, namespace pinned to 'acme'");
    let (shape_nonexistent_tool, durations_nonexistent_tool) = probe(&client_b, qualified, 200).await;

    // Case 3: existing namespace, existing tool, never shared (private).
    let owner_client = McpClient::with_bearer(&server.base_url, &owner_key);
    owner_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "lookup", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("acme publishes lookup, left private");
    let (shape_unshared, durations_unshared) = probe(&client_b, qualified, 200).await;

    assert_eq!(
        shape_nonexistent_ns, shape_nonexistent_tool,
        "nonexistent-namespace and nonexistent-tool responses must be byte-identical"
    );
    assert_eq!(
        shape_nonexistent_tool, shape_unshared,
        "nonexistent-tool and unshared-tool responses must be byte-identical"
    );

    let p95_a = p95_ms(durations_nonexistent_ns);
    let p95_b = p95_ms(durations_nonexistent_tool);
    let p95_c = p95_ms(durations_unshared);
    println!(
        "AC3 p95s (ms): nonexistent_ns={p95_a:.2} nonexistent_tool={p95_b:.2} unshared_tool={p95_c:.2}"
    );
    let values = [p95_a, p95_b, p95_c];
    let spread = values.iter().cloned().fold(f64::MIN, f64::max)
        - values.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        spread < 10.0,
        "p95 latency spread {spread:.2}ms across the three cases exceeds the 10ms budget \
         (nonexistent_ns={p95_a:.2} nonexistent_tool={p95_b:.2} unshared_tool={p95_c:.2})"
    );
}
