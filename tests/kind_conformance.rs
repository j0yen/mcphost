//! AC17 — Given the conformance suite, When run against the `echo` kind,
//! Then it passes; Given a `Kind` whose `describe` returns an invalid
//! schema, Then the suite fails naming `describe`.
//!
//! This is *the* conformance suite the PRD requires at `tests/kind_conformance.rs`
//! (requirement 5): `mcphost::kinds::conformance` is the reusable checker;
//! the feature-PRD crates that extend this registry (REST-wrapper, code
//! kinds) run the same checker against their own kinds in their own copy of
//! this file.

use async_trait::async_trait;
use mcphost::kinds::conformance::{check_call, check_schema};
use mcphost::kinds::echo::EchoKind;
use mcphost::kinds::{CallCtx, Kind, KindError, ToolDescriptor};
use serde_json::{Value, json};

#[test]
fn echo_kind_passes_schema_conformance() {
    let spec = json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}});
    check_schema(&EchoKind, &spec).expect("echo kind must pass schema conformance");
}

#[tokio::test]
async fn echo_kind_passes_call_conformance() {
    let spec = json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}});
    check_call(&EchoKind, &spec, json!({"msg": "hi"}), json!({"msg": 5}))
        .await
        .expect("echo kind must pass call conformance");
}

/// A deliberately broken kind: `describe` returns a schema whose `type`
/// value is a number, which is not valid JSON Schema.
struct BadDescribeKind;

#[async_trait]
impl Kind for BadDescribeKind {
    fn name(&self) -> &'static str {
        "bad_describe"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "bad".to_string(),
            description: "a kind whose schema is broken".to_string(),
            input_schema: json!({"type": 123}),
        }
    }

    async fn call(&self, _spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        Ok(args)
    }
}

#[test]
fn suite_fails_naming_describe_for_an_invalid_schema() {
    let err = check_schema(&BadDescribeKind, &json!({}))
        .expect_err("a kind with an invalid describe() schema must fail conformance");
    assert_eq!(
        err.method, "describe",
        "the conformance failure must name the offending method as `describe`, got: {err}"
    );
}
