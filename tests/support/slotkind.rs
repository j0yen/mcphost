//! Test-only `slot` kind for the `cest_ac*` tests (PRD-mcphost-call-cost-estimate):
//! a kind with a small admission semaphore, standing in for the python kind's
//! per-tenant `concurrent_calls_per_tenant` cap without needing a sandbox.
//! `call` acquires a permit (refusing `capacity` when none is free) and holds
//! it for the spec's `hold_ms`; `admission_capacity` only READS the free
//! permits, which is exactly the contract a dry run relies on.

#![allow(dead_code)]

use async_trait::async_trait;
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

pub struct SlotKind {
    sem: Arc<Semaphore>,
}

fn capacity_error() -> KindError {
    KindError::structured_with(
        "capacity",
        "at this tenant's concurrent-call limit; try again shortly",
        json!({"retry_after_ms": 100, "scope": "tenant"}),
    )
}

impl SlotKind {
    pub fn new(permits: usize) -> Arc<Self> {
        Arc::new(Self { sem: Arc::new(Semaphore::new(permits)) })
    }

    pub fn semaphore(&self) -> Arc<Semaphore> {
        self.sem.clone()
    }

    pub fn registry(self: &Arc<Self>) -> KindRegistry {
        let mut kinds = KindRegistry::with_builtin();
        kinds.register(self.clone() as Arc<dyn Kind>);
        kinds
    }
}

#[async_trait]
impl Kind for SlotKind {
    fn name(&self) -> &'static str {
        "slot"
    }

    fn idempotent_default(&self, _spec: &serde_json::Value) -> bool {
        false
    }

    fn known_spec_fields(&self) -> &'static [&'static str] {
        &["schema", "hold_ms"]
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "slot".to_string(),
            description: "holds a slot for hold_ms".to_string(),
            input_schema: spec.get("schema").cloned().unwrap_or_else(|| json!({"type": "object"})),
        }
    }

    async fn call(&self, spec: &Value, _args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        let Ok(_permit) = self.sem.clone().try_acquire_owned() else {
            return Err(capacity_error());
        };
        if let Some(ms) = spec.get("hold_ms").and_then(Value::as_u64) {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        Ok(json!({"ok": true}))
    }

    fn admission_capacity(&self, _tenant_id: i64, _cap: usize) -> Option<KindError> {
        (self.sem.available_permits() == 0).then(capacity_error)
    }
}
