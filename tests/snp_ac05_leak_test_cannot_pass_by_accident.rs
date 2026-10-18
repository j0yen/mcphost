//! PRD mcphost-secret-non-exfiltration-proof AC5 — Given a build with the outbound check disabled (test feature flag),
//! When `host.secret.leak_test` runs, Then `egress: "LEAKED"` and the call
//! fails `leak_test_failed` — the self-test cannot pass by accident.

use crate::common;
use common::{FixedLookup, McpClient, TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::{HttpKind, NameLookup};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

fn registry(outbound_check_disabled: bool) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    let lookup: Arc<dyn NameLookup> = Arc::new(FixedLookup(HashMap::new()));
    let kind = HttpKind::for_test("127.0.0.1", lookup);
    let kind = if outbound_check_disabled { kind.with_outbound_check_disabled() } else { kind };
    kinds.register(Arc::new(kind));
    kinds
}

async fn run_leak_test(outbound_check_disabled: bool) -> Result<serde_json::Value, common::RpcError> {
    let server = TestServer::start_with_kinds(registry(outbound_check_disabled)).await;
    let (_ns, key) = signup(&server.base_url, "Leak Test Flag Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.secret.set",
            json!({"name": "x", "value": "sk_live_canary_secret_value", "hosts": ["api.example.com"]}),
        )
        .await
        .expect("secret set ok");
    client.tools_call("host.secret.leak_test", json!({"name": "x"})).await
}

#[tokio::test]
async fn disabled_outbound_check_reports_leaked_and_fails_the_call() {
    let err = run_leak_test(true).await.expect_err("a missing block must fail the self-test");
    assert_eq!(err.error_code.as_deref(), Some("leak_test_failed"), "{err:?}");
    assert_eq!(err.data["egress"], "LEAKED", "{err:?}");
    assert_eq!(err.data["echo"], "blocked", "the result-side block is still on: {err:?}");
    assert!(!err.data.to_string().contains("sk_live_canary_secret_value"));
}

#[tokio::test]
async fn the_same_build_with_the_check_enabled_passes() {
    let ok = run_leak_test(false).await.expect("self-test passes with the check on");
    let report = common::extract_structured(&ok);
    assert_eq!(report["egress"], "blocked", "{report}");
}
