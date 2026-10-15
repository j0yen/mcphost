//! AC4 (P0) — Given `host.tool_call` on a python tool that raises, When the
//! `tool_exception` error returns, Then `feedback_hint` contains
//! `context:{error_code:"tool_exception", tool:"<tool name>"}` and
//! `data.traceback` is still present.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, signup};
use mcphost::errors::AppError;
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

/// Stands in for the python kind's raised-exception path (the real one
/// needs a userns sandbox and a pip env build, which not every runner has):
/// returns exactly the `KindError` `map_envelope_error` builds for an
/// exception that escapes the tool's code.
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

    async fn call(&self, _spec: &Value, _args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        Err(KindError::structured_with(
            "tool_exception",
            "ValueError: boom at tool.py:2",
            json!({
                "phase": "tool_code",
                "exception_class": "ValueError",
                "traceback": "Traceback (most recent call last):\nValueError: boom",
            }),
        ))
    }
}

#[test]
fn a_tool_exception_with_a_call_site_tool_names_it_and_keeps_its_data() {
    let err = AppError::from(KindError::structured_with(
        "tool_exception",
        "ValueError: boom".to_string(),
        json!({"traceback": "Traceback ...", "exception_class": "ValueError"}),
    ))
    .with_tool("raiser");
    let data = err.into_error_data().data.unwrap();
    let hint = data["feedback_hint"].as_str().unwrap();
    assert!(
        hint.contains("context:{error_code:\"tool_exception\", tool:\"raiser\"}"),
        "{hint}"
    );
    assert_eq!(data["traceback"], json!("Traceback ..."));
}

#[tokio::test]
async fn host_tool_call_on_a_raising_tool_hints_with_the_tool_name() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(RaiseKind));
    let server =
        TestServer::start_full(Some(common::ADMIN_KEY.to_string()), kinds, Duration::from_secs(5), None)
            .await;
    let (ns, key) = signup(&server.base_url, "FBEH AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "raiser", "kind": "raise", "spec": {}}))
        .await
        .expect("publish ok");

    let err = client
        .tools_call("host.tool_call", json!({"name": format!("{ns}.raiser"), "args": {}}))
        .await
        .expect_err("a raised exception must not succeed");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"));
    let hint = err.data["feedback_hint"].as_str().expect("feedback_hint");
    assert!(
        hint.contains("context:{error_code:\"tool_exception\", tool:\"raiser\"}"),
        "{hint}"
    );
    assert!(err.data["traceback"].as_str().unwrap_or("").contains("ValueError"));
}
