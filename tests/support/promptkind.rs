//! Test-only `prompt` kind for the `mprm_ac*` tests
//! (PRD-mcphost-prompts-from-published-tools): a tool whose spec is a
//! `template` with `{{slot}}`s and an `args_schema`. `render_prompt` is the
//! dry-run renderer behind `prompts/get`; `call` stands in for the provider
//! and bumps a counter, so a test proves `prompts/get` never reaches it.

#![allow(dead_code)]

use async_trait::async_trait;
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct PromptKind {
    provider_calls: AtomicUsize,
}

impl PromptKind {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { provider_calls: AtomicUsize::new(0) })
    }

    /// How many times the (fixture) provider was reached.
    pub fn provider_calls(&self) -> usize {
        self.provider_calls.load(Ordering::SeqCst)
    }

    pub fn registry(self: &Arc<Self>) -> KindRegistry {
        let mut kinds = KindRegistry::with_builtin();
        kinds.register(self.clone() as Arc<dyn Kind>);
        kinds
    }
}

impl PromptKind {
    /// [`PromptKind::registry`] plus a `python` kind rooted at `data_dir`.
    pub fn registry_with_python(self: &Arc<Self>, data_dir: &std::path::Path) -> KindRegistry {
        let mut kinds = self.registry();
        kinds.register(Arc::new(mcphost::kinds::python::PythonKind::new(data_dir)));
        kinds
    }
}

/// The spec the tests publish: `text` required, `labels` optional.
pub fn classify_spec() -> Value {
    json!({
        "template": "Classify this ticket: {{text}} using the labels: {{labels}}",
        "args_schema": {
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "the ticket body"},
                "labels": {"type": "string"},
            },
            "required": ["text"],
        },
    })
}

#[async_trait]
impl Kind for PromptKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "prompt"
    }

    fn known_spec_fields(&self) -> &'static [&'static str] {
        &["template", "args_schema"]
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        spec.get("template")
            .and_then(Value::as_str)
            .map(|_| ())
            .ok_or_else(|| KindError::InvalidSpec("template: required string".to_string()))
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "prompt".to_string(),
            description: "a reusable prompt template".to_string(),
            input_schema: spec.get("args_schema").cloned().unwrap_or_else(|| json!({"type": "object"})),
        }
    }

    fn render_prompt(&self, spec: &Value, args: &Map<String, Value>) -> Option<Result<String, KindError>> {
        let mut text = spec.get("template")?.as_str()?.to_string();
        for (name, value) in args {
            let v = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
            text = text.replace(&format!("{{{{{name}}}}}"), &v);
        }
        Some(Ok(text))
    }

    async fn call(&self, _spec: &Value, _args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        self.provider_calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"ok": true}))
    }
}
