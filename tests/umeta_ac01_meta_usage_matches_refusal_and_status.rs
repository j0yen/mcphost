//! PRD-mcphost-usage-in-every-response AC1 -- Given a free tenant and 501
//! sequential calls, When each result and the final refusal are read, Then
//! `_meta.usage.calls_per_day.used` is N after call N for all 500, the 501st
//! refusal's `data.used` is 500, and `billing.status` read between any two
//! calls reports the same `used` (one function, asserted by name).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

/// The one function every surface reads. Binding it by path makes the test
/// fail to compile if it is renamed or moved.
#[allow(dead_code)]
const SNAPSHOT_FN: &str = "plan_usage_snapshot";

fn source(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[tokio::test]
async fn meta_usage_used_is_n_after_call_n_and_refusal_and_status_agree() {
    // By name: the function exists, and the refusal, `billing.status`, and
    // the result builder all call it -- no second copy of the count.
    let _snapshot = mcphost::usage::plan_usage_snapshot;
    for rel in ["src/preflight.rs", "src/billing.rs", "src/usage.rs"] {
        assert!(source(rel).contains(&format!("{SNAPSHOT_FN}(")), "{rel} must call {SNAPSHOT_FN}");
    }
    assert!(
        source("src/usage.rs").contains("pub async fn result_usage") && source("src/usage.rs").contains("plan_usage_snapshot(state, billed"),
        "_meta.usage must be built by plan_usage_snapshot"
    );

    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Meta Usage Drift").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let qualified = format!("{ns}.hello");

    let limit = 500;
    for n in 1..=limit {
        let result = client.tools_call(&qualified, json!({"n": n})).await.unwrap_or_else(|e| panic!("call {n}: {e:?}"));
        let block = &result["_meta"]["usage"]["calls_per_day"];
        assert_eq!(block["used"].as_i64(), Some(n), "call {n}: _meta.usage {result}");
        assert_eq!(block["limit"].as_i64(), Some(limit));
        assert_eq!(block["remaining"].as_i64(), Some(limit - n));
        if n % 50 == 0 || n == limit {
            let status = client.tools_call("billing.status", json!({})).await.expect("billing.status");
            let status = extract_structured(&status);
            assert_eq!(
                status["usage"]["calls_per_day"]["used"].as_i64(),
                Some(n),
                "billing.status after call {n}: {status}"
            );
        }
    }

    let refused = client.tools_call(&qualified, json!({"n": 501})).await.expect_err("501st call refused");
    assert_eq!(refused.error_code.as_deref(), Some("quota_exceeded"), "{refused:?}");
    assert_eq!(refused.data["used"].as_i64(), Some(limit), "refusal data.used: {:?}", refused.data);
}
