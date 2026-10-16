//! PRD-mcphost-acting-for-in-ledger-and-audit
//! AC6 (P1) — Given a child call that raises `tool_exception`, When
//! `host.tool.logs` is read, Then the line is prefixed
//! `[acting_for=deployer<lead-job-42]` and the stored line in the DB has no
//! prefix.

use crate::common::{self, McpClient, TestServer, extract_structured, signup};
use async_trait::async_trait;
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// Logs one line, then raises exactly what an escaped python exception maps to.
struct RaiseKind;

#[async_trait]
impl Kind for RaiseKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "raise"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "raise".to_string(),
            description: "always raises".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn call(&self, _spec: &Value, _args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        ctx.log.log("ValueError: boom at tool.py:2");
        Err(KindError::structured_with(
            "tool_exception",
            "ValueError: boom at tool.py:2",
            json!({"phase": "tool_code", "exception_class": "ValueError", "traceback": "Traceback\nValueError: boom"}),
        ))
    }
}

async fn delegate(base: &str, client: &McpClient, label: &str, ttl_s: i64) -> McpClient {
    let out: Value = extract_structured(
        &client
            .tools_call(
                "host.key.delegate",
                json!({"scopes": ["tools:call"], "ttl_s": ttl_s, "label": label, "can_delegate": true}),
            )
            .await
            .unwrap_or_else(|e| panic!("delegate {label}: {e:?}")),
    );
    McpClient::with_bearer(base, out["key"].as_str().expect("key"))
}

#[tokio::test]
async fn actfor_ac06_child_log_lines_render_a_prefix_and_store_none() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(RaiseKind));
    let server =
        TestServer::start_full(Some(common::ADMIN_KEY.to_string()), kinds, Duration::from_secs(5), None).await;
    let (ns, key) = signup(&server.base_url, "ActFor Logs Tenant").await;
    let primary = McpClient::with_bearer(&server.base_url, &key);
    primary
        .tools_call("host.tool_publish", json!({"name": "raiser", "kind": "raise", "spec": {}}))
        .await
        .expect("publish");
    let lead = delegate(&server.base_url, &primary, "lead-job-42", 3000).await;
    let deployer = delegate(&server.base_url, &lead, "deployer", 2000).await;

    let err = deployer.tools_call(&format!("{ns}.raiser"), json!({})).await.expect_err("raises");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"), "{err:?}");
    primary.tools_call(&format!("{ns}.raiser"), json!({})).await.expect_err("primary raises too");

    let logs = extract_structured(&primary.tools_call("host.tool_logs", json!({"name": "raiser"})).await.expect("logs"));
    let lines: Vec<&str> = logs["lines"].as_array().expect("lines").iter().map(|l| l.as_str().expect("str")).collect();
    assert_eq!(
        lines,
        ["[acting_for=deployer<lead-job-42] ValueError: boom at tool.py:2", "ValueError: boom at tool.py:2"],
        "{logs}"
    );

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let mut stmt = conn.prepare("SELECT line FROM logs WHERE tool_name = 'raiser' ORDER BY id").expect("prepare");
    let stored: Vec<String> =
        stmt.query_map([], |r| r.get(0)).expect("query").collect::<Result<_, _>>().expect("rows");
    assert_eq!(stored, ["ValueError: boom at tool.py:2", "ValueError: boom at tool.py:2"], "stored lines carry no prefix");
}
