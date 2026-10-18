//! PRD-mcphost-retry-policy-table-generated AC1 (falsifier) — Given two
//! fake-mode arms of 20 persona sessions with a planted tool that always
//! answers `args_invalid`, When arm A's error carries `retry {retryable:
//! false}` and arm B's carries none, Then arm A's identical re-sends are
//! <= 50 % of arm B's.
//!
//! The planted tool is real (published, called over MCP), so arm A reads the
//! host's own `data.retry`. Arm B is the same error with the `retry` key
//! removed, the host as it was before this PRD. A fake persona follows the
//! one rule the block teaches: a `retryable: false` error means fix the
//! argument (a changed call, not an identical re-send); an error with no
//! `retry` block gets the same call again, up to MAX_SENDS.

use crate::common;
use async_trait::async_trait;
use common::{McpClient, TestServer, signup};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;

const MAX_SENDS: usize = 5;

struct PlantedArgsInvalidKind;

#[async_trait]
impl Kind for PlantedArgsInvalidKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "rpol_planted_args_invalid"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "planted".to_string(),
            description: "always answers args_invalid".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        if args.get("fixed").is_some() {
            return Ok(json!({"ok": true}));
        }
        Err(KindError::structured("args_invalid", "the planted argument is always wrong"))
    }
}

/// One persona session; returns its count of identical re-sends.
async fn session(client: &McpClient, tool: &str, persona: usize, strip_retry: bool) -> usize {
    let args = json!({"persona": persona});
    let mut identical_resends = 0;
    for send in 0..MAX_SENDS {
        let err = client.tools_call(tool, args.clone()).await.expect_err("the planted tool always fails");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
        let mut data = err.data.clone();
        if strip_retry {
            data.as_object_mut().expect("error data is an object").remove("retry");
        } else {
            assert_eq!(data["retry"]["retryable"], json!(false), "host carries retry on args_invalid: {data}");
        }
        if data["retry"]["retryable"] == json!(false) {
            client
                .tools_call(tool, json!({"persona": persona, "fixed": true}))
                .await
                .expect("the fixed call succeeds");
            return identical_resends;
        }
        if send + 1 < MAX_SENDS {
            identical_resends += 1;
        }
    }
    identical_resends
}

#[tokio::test]
async fn rpol_ac01() {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(PlantedArgsInvalidKind));
    let server = TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "Rpol Ac01").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "planted", "kind": "rpol_planted_args_invalid", "spec": {}}))
        .await
        .expect("publish planted tool");
    let tool = format!("{ns}.planted");

    let mut arm_a = 0;
    let mut arm_b = 0;
    for i in 0..20 {
        arm_a += session(&client, &tool, i, false).await;
        arm_b += session(&client, &tool, 100 + i, true).await;
    }
    assert!(arm_b > 0, "the control arm must re-send identically (got {arm_b})");
    assert!(arm_a * 2 <= arm_b, "falsifier: arm A re-sent {arm_a} times vs arm B {arm_b}; premise does not hold");
}
