//! PRD-mcphost-job-timeline-tool AC4 (P0) -- Given a chain run still
//! executing step 3, When the timeline is read, Then `run.status` is
//! `running`, `steps[2].status` is `running` with `finished_unix: null`, and
//! `totals.running` is 1.
//!
//! A test-only `slow` kind (same pattern as `budget_ac05`) holds step 3 open
//! long enough for the poll to land inside it.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor, chain::ChainKind};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct SlowEchoKind;

#[async_trait]
impl Kind for SlowEchoKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "slow"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "slow".to_string(),
            description: "echoes its args back after a delay".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        Ok(args)
    }
}

#[tokio::test]
async fn jtl_ac04() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(ChainKind));
    kinds.register(Arc::new(SlowEchoKind));
    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "JTL4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "step1", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "step2", "echo", json!({"schema": {"type": "object"}})).await;
    publish(&client, "step3", "slow", json!({})).await;
    publish(
        &client,
        "pipeline",
        "chain",
        json!({"steps": [
            {"tool": "step1", "args": {"n": 1}},
            {"tool": "step2", "args": {"n": 2}},
            {"tool": "step3", "args": {"n": 3}}
        ]}),
    )
    .await;
    let enqueue = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": "pipeline", "args": {}, "async": true}))
            .await
            .expect("enqueue"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut observed = None;
    while Instant::now() < deadline {
        let timeline = extract_structured(
            &client.tools_call("host.job.timeline", json!({"run_id": run_id})).await.expect("timeline"),
        );
        if timeline["steps"].as_array().is_some_and(|s| s.len() == 3 && s[2]["status"] == json!("running")) {
            observed = Some(timeline);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let timeline = observed.expect("never observed step 3 running within 20s");

    assert_eq!(timeline["run"]["status"], json!("running"), "{timeline}");
    let steps = timeline["steps"].as_array().unwrap();
    assert_eq!(steps[0]["status"], json!("done"), "{timeline}");
    assert_eq!(steps[1]["status"], json!("done"), "{timeline}");
    assert_eq!(steps[2]["step"], json!(3), "{timeline}");
    assert!(steps[2]["finished_unix"].is_null(), "{timeline}");
    assert!(steps[2]["duration_ms"].is_null(), "{timeline}");
    assert!(steps[2]["started_unix"].is_i64(), "{timeline}");
    assert_eq!(timeline["totals"]["running"], json!(1), "{timeline}");
    assert_eq!(timeline["totals"]["done"], json!(2), "{timeline}");
}
