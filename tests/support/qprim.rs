//! Shared harness for the PRD-mcphost-queue-primitive-v2 tests: a server with a
//! sleeping consumer kind, the real executor, and (on request) the queue
//! dispatcher. Pulled in per-file with `#[path = "support/qprim.rs"] mod qprim;`.

#![allow(dead_code)]

use crate::common::{self, McpClient, RpcError, TempDataDir, extract_structured, signup};
use async_trait::async_trait;
use mcphost::db::queues::{QueueItemRow, SeedItem};
use mcphost::kinds::{CallCtx, Kind, KindError, KindRegistry, ToolDescriptor};
use mcphost::plans::PlanCatalog;
use mcphost::state::AppState;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// `qslow`: input schema is `spec.schema` (default any object), each call sleeps
/// `spec.sleep_ms` (default 0) and echoes its arguments.
pub struct QSlowKind;

#[async_trait]
impl Kind for QSlowKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }

    fn name(&self) -> &'static str {
        "qslow"
    }

    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    fn known_spec_fields(&self) -> &'static [&'static str] {
        &["schema", "sleep_ms"]
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "qslow".to_string(),
            description: "sleeps spec.sleep_ms, then echoes its arguments".to_string(),
            input_schema: spec.get("schema").cloned().unwrap_or_else(|| json!({"type": "object"})),
        }
    }

    async fn call(&self, spec: &Value, args: Value, _ctx: &CallCtx) -> Result<Value, KindError> {
        let ms = spec.get("sleep_ms").and_then(Value::as_u64).unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(json!({"ok": true, "item": args}))
    }
}

/// What a test drives: the state (for direct ticks and plan reads), the
/// first tenant's client and namespace.
pub struct Rig {
    pub base_url: String,
    pub state: Arc<AppState>,
    pub client: McpClient,
    pub ns: String,
    _data_dir: TempDataDir,
}

/// Background tasks a rig starts beside the HTTP listener.
#[derive(Clone, Copy)]
pub struct Tasks {
    pub executor: bool,
    pub dispatcher: bool,
}

pub const ALL_TASKS: Tasks = Tasks { executor: true, dispatcher: true };

/// No background task: the test drives `queues::tick` itself.
pub const NO_TASKS: Tasks = Tasks { executor: false, dispatcher: false };

/// A server over `bare_app_state` with the `qslow` kind, `mutate` applied to the plan catalog.
pub async fn rig(tasks: Tasks, mutate: impl FnOnce(&mut PlanCatalog)) -> Rig {
    let (mut state, data_dir) = common::bare_app_state().await;
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(QSlowKind));
    state.kinds = kinds;
    mutate(&mut state.plans);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let base_url = format!("http://{addr}");
    state.public_url = base_url.clone();
    let state = Arc::new(state);
    if tasks.executor {
        mcphost::runs::spawn_executor((*state).clone());
    }
    if tasks.dispatcher {
        mcphost::queues::spawn_dispatcher((*state).clone());
    }
    let serve_state = state.clone();
    tokio::spawn(async move {
        let _ = mcphost::http::serve_on_listener(listener, serve_state).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (ns, key) = signup(&base_url, "qprim tenant").await;
    let client = McpClient::with_bearer(&base_url, &key);
    Rig { base_url, state, client, ns, _data_dir: data_dir }
}

impl Rig {
    /// The queue's items in push order (empty when the queue does not exist).
    pub async fn item_rows(&self, queue: &str) -> Vec<QueueItemRow> {
        let tenant = self.state.db.find_tenant_by_namespace(self.ns.clone()).await.expect("tenant lookup").expect("tenant");
        match self.state.db.queue_get(tenant.id, queue.to_string()).await.expect("queue lookup") {
            Some(q) => self.item_rows_by_queue_id(q.id).await,
            None => Vec::new(),
        }
    }

    async fn queue_id(&self, queue: &str) -> i64 {
        let tenant = self.state.db.find_tenant_by_namespace(self.ns.clone()).await.expect("tenant lookup").expect("tenant");
        self.state.db.queue_get(tenant.id, queue.to_string()).await.expect("queue lookup").expect("queue").id
    }

    /// An item stored as `running` under a run id that has no `runs` row.
    pub async fn seed_orphan(&self, queue: &str, run_id: &str) {
        let seed = SeedItem { item_json: "{}".to_string(), status: "running", run_id: Some(run_id.to_string()), ended_at_ms: None };
        self.state.db.queue_test_insert_item(self.queue_id(queue).await, seed).await.expect("seed orphan");
    }

    /// An item stored in `status`, ended at `ended_at_ms` when it has finished.
    pub async fn seed_item(&self, queue: &str, status: &'static str, ended_at_ms: Option<i64>) {
        let run_id = (status == "running").then(|| format!("seeded_{queue}"));
        let seed = SeedItem { item_json: "{}".to_string(), status, run_id, ended_at_ms };
        self.state.db.queue_test_insert_item(self.queue_id(queue).await, seed).await.expect("seed item");
    }

    pub async fn item_rows_by_queue_id(&self, queue_id: i64) -> Vec<QueueItemRow> {
        self.state.db.queue_item_rows(queue_id).await.expect("item rows")
    }
}

/// Sets one numeric field of the `free` plan.
pub fn free_plan(catalog: &mut PlanCatalog, edit: impl FnOnce(&mut mcphost::plans::Plan)) {
    edit(catalog.plans.iter_mut().find(|p| p.name == "free").expect("free plan"));
}

/// A second tenant on the same server.
pub async fn second_tenant(rig: &Rig) -> McpClient {
    let (_ns, key) = signup(&rig.base_url, "qprim tenant b").await;
    McpClient::with_bearer(&rig.base_url, &key)
}

/// One call, structured result; panics on an error.
pub async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool} failed: {e:?}")))
}

/// One call that must fail.
pub async fn call_err(client: &McpClient, tool: &str, args: Value) -> RpcError {
    match client.tools_call(tool, args).await {
        Ok(v) => panic!("{tool} should have failed, got {v}"),
        Err(e) => e,
    }
}

/// Publishes a `qslow` tool called `name`.
pub async fn publish_consumer(client: &McpClient, name: &str, schema: Value, sleep_ms: u64) {
    call(
        client,
        "host.tool_publish",
        json!({"name": name, "kind": "qslow", "spec": {"schema": schema, "sleep_ms": sleep_ms}}),
    )
    .await;
}

/// The `any object` consumer schema.
pub fn any_object() -> Value {
    json!({"type": "object"})
}

pub async fn stats(client: &McpClient, queue: &str) -> Value {
    call(client, "host.queue.stats", json!({"name": queue})).await
}

/// Polls `stats` until `done` says so, or fails after `secs`.
pub async fn wait_stats(client: &McpClient, queue: &str, secs: u64, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let s = stats(client, queue).await;
        if done(&s) {
            return s;
        }
        assert!(Instant::now() < deadline, "queue {queue} never reached the wanted state: {s}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// `n` items `{"n": i}`.
pub fn items(n: usize) -> Value {
    Value::Array((0..n).map(|i| json!({"n": i})).collect())
}
