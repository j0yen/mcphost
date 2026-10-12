//! Shared fixture for the `ctrace_ac*` tests (PRD-mcphost-call-trace-replay):
//! a python-capable server and one signed-up tenant, plus helpers that
//! publish a python tool, call it through `host.tool_call` once its
//! environment is built, and read a trace back.

#![allow(dead_code)]

use crate::common::{McpClient, RpcError, TempDataDir, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use serde_json::{Value, json};
use std::time::Duration;

/// Call tracing is one process-wide switch (`calltrace::set_enabled`), and
/// the suite binary runs tests in parallel threads: any test that reads a
/// trace holds this for its whole body, and the AC1 falsifier holds it while
/// it flips the switch.
static TRACE_SWITCH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn trace_switch() -> tokio::sync::MutexGuard<'static, ()> {
    let guard = TRACE_SWITCH.lock().await;
    mcphost::calltrace::set_enabled(true);
    guard
}

pub struct Fixture {
    pub server: TestServer,
    pub client: McpClient,
    pub ns: String,
    pub key: String,
    _envs: TempDataDir,
}

pub async fn start() -> Fixture {
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "ctrace tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    Fixture { server, client, ns, key, _envs: envs }
}

/// [`start`], with the tenant on the pro plan (the free plan's 500
/// calls/day would end the AC1 falsifier's 1,000 calls early).
pub async fn start_pro() -> Fixture {
    let fx = start().await;
    let id = fx.tenant_id().await;
    fx.server
        .state
        .db
        .upgrade_tenant_plan(id, "pro".to_string(), mcphost::state::rfc3339_now(), None)
        .await
        .expect("upgrade to pro");
    fx
}

impl Fixture {
    pub async fn tenant_id(&self) -> i64 {
        self.server
            .state
            .db
            .find_tenant_by_namespace(self.ns.clone())
            .await
            .expect("tenant lookup")
            .expect("tenant exists")
            .id
    }

    pub async fn publish_python(&self, name: &str, source: &str) {
        self.client
            .tools_call(
                "host.tool_publish",
                json!({"name": name, "kind": "python", "spec": {"source": source}}),
            )
            .await
            .unwrap_or_else(|e| panic!("publish {name}: {} {}", e.code, e.message));
    }

    /// Publishes a python tool from a full spec (`source` plus whatever else
    /// the test needs, e.g. `args_schema`).
    pub async fn publish_python_spec(&self, name: &str, spec: Value) {
        self.client
            .tools_call("host.tool_publish", json!({"name": name, "kind": "python", "spec": spec}))
            .await
            .unwrap_or_else(|e| panic!("publish {name}: {} {}", e.code, e.message));
    }

    /// Directly calls the published tool until its environment is built, so
    /// the next call is a warm one.
    pub async fn warm(&self, name: &str, args: Value) {
        let _ = poll_until_ready(&self.client, &format!("{}.{name}", self.ns), args, Duration::from_secs(60)).await;
    }

    /// `host.tool_call`, structured result or the RPC error.
    pub async fn tool_call(&self, name: &str, args: Value) -> Result<Value, RpcError> {
        self.client
            .tools_call("host.tool_call", json!({"name": name, "args": args}))
            .await
            .map(|v| extract_structured(&v))
    }

    pub async fn trace(&self, client: &McpClient, call_id: &str) -> Result<Value, RpcError> {
        client
            .tools_call("host.call_trace", json!({"call_id": call_id}))
            .await
            .map(|v| extract_structured(&v))
    }
}

/// The 95th percentile (nearest rank) of `samples`, in milliseconds.
pub fn p95_ms(samples: &mut [f64]) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    let rank = ((samples.len() as f64) * 0.95).ceil() as usize;
    samples[rank.saturating_sub(1).min(samples.len() - 1)]
}
