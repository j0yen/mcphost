//! AC6 (PRD-mcphost-plan-limits-generated) — Given a published tool with
//! `timeout_s` above the caller plan's latency ceiling, When `host.tool_test`
//! runs, Then the result carries a warning naming both numbers.
//!
//! Uses a hermetic stand-in kind that declares its timeout from
//! `spec.timeout_s` the way `python` does, so no sandbox is needed.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use mcphost::plans::PlanCatalog;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

struct DeclaredTimeoutKind;

#[async_trait]
impl Kind for DeclaredTimeoutKind {
    fn name(&self) -> &'static str {
        "declared_timeout"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "declared_timeout".to_string(),
            description: "returns immediately".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn known_spec_fields(&self) -> &'static [&'static str] {
        &["timeout_s"]
    }

    fn requested_timeout(&self, spec: &Value) -> Option<Duration> {
        spec.get("timeout_s").and_then(Value::as_u64).map(Duration::from_secs)
    }

    async fn call(&self, _spec: &Value, _args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        Ok(json!({"ok": true}))
    }
}

async fn tested(client: &McpClient, name: &str, timeout_s: u64) -> Value {
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": name, "kind": "declared_timeout", "spec": {"timeout_s": timeout_s}}),
        )
        .await
        .expect("publish ok");
    extract_structured(
        &client
            .tools_call("host.tool_test", json!({"name": name, "args": {}}))
            .await
            .expect("tool_test ok"),
    )
}

#[tokio::test]
async fn tool_test_warns_naming_timeout_and_plan_ceiling() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(DeclaredTimeoutKind));
    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "PlanLim AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let free = PlanCatalog::default_catalog().get("free").unwrap().clone();
    let ceiling_ms = free.budget_max_tool_latency_ms;
    let over_s = (ceiling_ms as u64) / 1000 + 15;

    let over = tested(&client, "slow", over_s).await;
    let warnings = over["warnings"].as_array().expect("warnings array");
    assert_eq!(warnings.len(), 1, "{over}");
    let text = warnings[0].as_str().unwrap();
    assert!(text.contains(&format!("{}", over_s * 1000)), "names the tool timeout: {text}");
    assert!(text.contains(&ceiling_ms.to_string()), "names the plan ceiling: {text}");

    let under = tested(&client, "quick", 5).await;
    assert!(under.get("warnings").is_none(), "no warning at or under the ceiling: {under}");
}
