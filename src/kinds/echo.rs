//! The `echo` kind: `spec` names a JSON Schema, and a call returns whatever
//! arguments it was given (after validating them against that schema). It
//! exists so the harness can measure the bootstrap path — signup, publish,
//! call — end to end without any real integration behind it.

use serde_json::{Value, json};

use super::{CallCtx, Kind, KindError, ToolDescriptor};

pub struct EchoKind;

fn schema_of(spec: &Value) -> Value {
    spec.get("schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}))
}

#[async_trait::async_trait]
impl Kind for EchoKind {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        if !spec.is_object() {
            return Err(KindError::InvalidSpec("spec must be a JSON object".into()));
        }
        let schema = spec
            .get("schema")
            .ok_or_else(|| KindError::InvalidSpec("spec.schema is required".into()))?;
        jsonschema::validator_for(schema).map_err(|e| {
            KindError::InvalidSpec(format!("spec.schema is not a valid JSON Schema: {e}"))
        })?;
        Ok(())
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "echo".to_string(),
            description: "Echoes back the arguments it was called with.".to_string(),
            input_schema: schema_of(spec),
        }
    }

    async fn call(&self, spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        let schema = schema_of(spec);
        let validator = jsonschema::validator_for(&schema)
            .map_err(|e| KindError::InvalidSpec(format!("spec.schema is not valid: {e}")))?;
        validator
            .validate(&args)
            .map_err(|e| KindError::InvalidArgs(e.to_string()))?;
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_returns_its_arguments() {
        let kind = EchoKind;
        let spec = json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}});
        kind.validate(&spec).expect("valid spec");
        let ctx = CallCtx::for_test(1, "t_deadbeef");
        let args = json!({"msg": "hi"});
        let result = kind.call(&spec, args.clone(), &ctx).await.expect("call ok"); // allowlist: test-only expect inside #[cfg(test)]
        assert_eq!(result, args);
    }

    #[tokio::test]
    async fn echo_rejects_args_outside_schema() {
        let kind = EchoKind;
        let spec = json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "required": ["msg"]}});
        let ctx = CallCtx::for_test(1, "t_deadbeef");
        let err = kind
            .call(&spec, json!({"nope": 1}), &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, KindError::InvalidArgs(_)));
    }

    #[test]
    fn validate_rejects_missing_schema() {
        let kind = EchoKind;
        let err = kind.validate(&json!({})).unwrap_err();
        assert!(matches!(err, KindError::InvalidSpec(_)));
    }
}
