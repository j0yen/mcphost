//! PRD-mcphost-run-budget-governor
//! AC5 (P0) — Given a running chain at 9 of 10 child calls, When
//! `host.runs.get` is polled, Then `progress_json.budget.fraction` is 0.9
//! and `verdict` is `alert`.
//!
//! A custom `slow` kind (same "test-only Kind registered alongside the
//! builtins" pattern as `tests/ac15_call_timeout.rs`'s `HangKind`) sleeps
//! briefly on every call so a ten-step chain takes long enough in wall
//! clock for this test to catch the run mid-flight, right after the ninth
//! child's record+persist and before the tenth's own dispatch completes.

use async_trait::async_trait;
use crate::common;
use common::{McpClient, TestServer, extract_structured, publish, signup};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor, chain::ChainKind};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct SlowEchoKind;

#[async_trait]
impl Kind for SlowEchoKind {
    fn name(&self) -> &'static str {
        "slow"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "slow".to_string(),
            description: "echoes its args back after a short delay".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        tokio::time::sleep(Duration::from_millis(400)).await;
        Ok(args)
    }
}

fn slow_and_chain_kind_registry() -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(ChainKind));
    kinds.register(Arc::new(SlowEchoKind));
    kinds
}

#[tokio::test]
async fn progress_reads_fraction_0_9_and_verdict_alert_mid_run() {
    let server = TestServer::start_with_kinds(slow_and_chain_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for i in 0..10 {
        publish(&client, &format!("step{i}"), "slow", json!({})).await;
    }
    let steps: Vec<Value> = (0..10).map(|i| json!({"tool": format!("step{i}"), "args": {"n": i}})).collect();
    let chain = publish(&client, "pipeline", "chain", json!({"steps": steps})).await;
    assert_eq!(chain, format!("{ns}.pipeline"));

    let enqueue = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({
                    "name": "pipeline",
                    "args": {},
                    "async": true,
                    "budget": {"max_child_calls": 10},
                }),
            )
            .await
            .expect("enqueue ok"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut observed = None;
    while Instant::now() < deadline {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("runs.get ok"),
        );
        let child_calls = got["progress"]["budget"]["used"]["child_calls"].as_i64();
        if child_calls == Some(9) {
            observed = Some(got);
            break;
        }
        if got["status"] != json!("queued") && got["status"] != json!("running") {
            panic!("run finalized ({}) before 9/10 was ever observed: {got}", got["status"]);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let got = observed.expect("never observed progress.budget.used.child_calls == 9 within 15s");
    let budget = &got["progress"]["budget"];
    let fraction = budget["fraction"].as_f64().expect("fraction is a number");
    assert!(
        (fraction - 0.9).abs() < 1e-9,
        "fraction must be 0.9 at 9/10 child calls: {budget}"
    );
    assert_eq!(budget["verdict"], json!("alert"), "{budget}");
    assert_eq!(got["status"], json!("running"), "{got}");
}
