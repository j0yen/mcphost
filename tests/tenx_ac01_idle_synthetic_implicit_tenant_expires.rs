//! PRD-mcphost-synthetic-implicit-tenant-expiry
//! AC1 (P0) — Given subject fixture A and seven variants (claimed, origin
//! external, idle 1 hour, paid, `synthetic` NULL, billing_ref set, before the
//! floor), When `prune_once` runs with the default window, Then only A ends
//! `disabled = 1, disabled_reason = 'idle_expired'`, its session released,
//! every kind's `on_tenant_removed` run; the variants are unchanged.

use crate::common::tenx::{self, DAY, Fx};
use crate::common::{EnvGuard, TestServer};
use async_trait::async_trait;
use mcphost::kinds::{Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

/// Records every tenant id `on_tenant_removed` is called with.
struct HookKind(Arc<Mutex<Vec<i64>>>);

#[async_trait]
impl Kind for HookKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }
    fn name(&self) -> &'static str {
        "tenxhook"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "tenxhook".to_string(),
            description: "records tenant removal".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(
        &self,
        _spec: &Value,
        _args: Value,
        _ctx: &mcphost::kinds::CallCtx,
    ) -> Result<Value, KindError> {
        Ok(json!({}))
    }
    async fn on_tenant_removed(&self, tenant_id: i64) {
        self.0.lock().unwrap().push(tenant_id);
    }
}

#[tokio::test]
async fn tenx_ac01_only_the_idle_synthetic_implicit_unclaimed_tenant_expires() {
    let _env = EnvGuard::clear(tenx::ENV).await;
    let removed = Arc::new(Mutex::new(Vec::new()));
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HookKind(removed.clone())));
    let server = TestServer::start_with_kinds(kinds).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    // Floor 10 days back, so every "after the floor" row is minted 3 days ago.
    tenx::set_floor(&conn, now - 10 * DAY);

    let idle = 2 * DAY;
    let a = tenx::insert(&conn, &Fx::subject("tenx-a", now, 3, idle));
    let mut variants: Vec<(&str, i64)> = Vec::new();
    let mut add = |label: &'static str, mutate: &dyn Fn(&mut Fx)| {
        let mut fx = Fx::subject(&format!("tenx-{label}"), now, 3, idle);
        mutate(&mut fx);
        variants.push((label, tenx::insert(&conn, &fx)));
    };
    add("claimed", &|f| f.claimed_unix = Some(now - DAY));
    add("external", &|f| f.origin = "external");
    add("idle-1h", &|f| f.last_seen_unix = Some(now - 3600));
    add("paid", &|f| f.paid_unix = Some(now - DAY));
    add("synthetic-null", &|f| f.synthetic = None);
    add("billing-ref", &|f| f.billing_ref = Some("cus_test"));
    add("before-floor", &|f| f.created_unix = now - 11 * DAY);

    let sid = server.state.session_bindings.issue();
    assert!(server.state.session_bindings.bind(&sid, a, now));
    assert_eq!(server.state.session_bindings.lookup(&sid, now), Some(a));

    server.state.db.prune_once().await.expect("prune_once");

    assert_eq!(tenx::state(&conn, a), (true, Some("idle_expired".to_string())));
    for (label, id) in &variants {
        assert_eq!(tenx::state(&conn, *id), (false, None), "variant {label} must be unchanged");
    }
    assert_eq!(server.state.session_bindings.lookup(&sid, now), None, "A's session is released");
    assert_eq!(*removed.lock().unwrap(), vec![a], "on_tenant_removed ran once, for A only");
}
