//! Shared fixture for the `ttpl_ac*` tests (PRD-mcphost-tenant-templates): a
//! server with the `jq` kind registered, one signed-up free tenant, and a call
//! log counting every tool call the test makes through [`Fixture::call`].

use std::sync::{Arc, Mutex};

use crate::common::{McpClient, RpcError, TestServer, extract_structured, signup};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

pub struct Fixture {
    pub server: TestServer,
    pub client: McpClient,
    pub namespace: String,
    log: Arc<Mutex<Vec<String>>>,
}

pub fn kinds() -> KindRegistry {
    let mut kinds = crate::common::http_kind_registry();
    kinds.register(Arc::new(mcphost::kinds::jq::JqKind));
    // Registered so `host.quickstart` (whose no-kind default is python) answers; no test here runs python.
    kinds.register(Arc::new(mcphost::kinds::python::PythonKind::new(&std::env::temp_dir().join("ttpl-python-kind"))));
    kinds
}

pub async fn start() -> Fixture {
    let server = TestServer::start_with_kinds(kinds()).await;
    let (namespace, key) = signup(&server.base_url, "ttpl tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    Fixture { server, client, namespace, log: Arc::new(Mutex::new(Vec::new())) }
}

impl Fixture {
    /// One logged tool call; the structured result, or the RPC error.
    pub async fn call(&self, tool: &str, args: Value) -> Result<Value, RpcError> {
        self.log.lock().expect("log").push(tool.to_string());
        self.client.tools_call(tool, args).await.map(|r| extract_structured(&r))
    }

    /// `call`, panicking with the error on failure.
    pub async fn ok(&self, tool: &str, args: Value) -> Value {
        self.call(tool, args).await.unwrap_or_else(|e| panic!("{tool} failed: {} {}", e.code, e.message))
    }

    pub fn calls(&self) -> Vec<String> {
        self.log.lock().expect("log").clone()
    }

    pub async fn apply(&self, name: &str) -> Result<Value, RpcError> {
        self.call("host.template_apply", json!({"name": name})).await
    }

    /// Names of the tenant's tools, trigger ids and table names, via calls the
    /// log does not count.
    pub async fn inventory(&self) -> (Vec<String>, Vec<String>, Vec<String>) {
        let get = |v: Value, key: &str, field: &str| -> Vec<String> {
            v[key].as_array().map(|a| a.iter().filter_map(|x| x[field].as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        let tools = extract_structured(&self.client.tools_call("host.tool_list", json!({})).await.expect("tool_list"));
        let triggers = extract_structured(&self.client.tools_call("host.trigger.list", json!({})).await.expect("trigger.list"));
        let tables = extract_structured(&self.client.tools_call("host.table.list", json!({})).await.expect("table.list"));
        (get(tools, "tools", "name"), get(triggers, "triggers", "id"), get(tables, "tables", "name"))
    }
}

/// Whether `have` carries every key/value of `want` (objects match by subset,
/// everything else by equality).
fn subset(have: &Value, want: &Value) -> bool {
    match (have, want) {
        (Value::Object(h), Value::Object(w)) => w.iter().all(|(k, v)| h.get(k).is_some_and(|x| subset(x, v))),
        _ => have == want,
    }
}

/// Whether `haystack` holds a value matching `needle` (see [`subset`]) anywhere
/// inside it.
pub fn contains_value(haystack: &Value, needle: &Value) -> bool {
    if subset(haystack, needle) {
        return true;
    }
    match haystack {
        Value::Array(a) => a.iter().any(|x| contains_value(x, needle)),
        Value::Object(m) => m.values().any(|x| contains_value(x, needle)),
        _ => false,
    }
}

impl Fixture {
    /// Gives the tenant `n` event triggers (each on its own echo tool), so a free
    /// plan's `event_triggers_max` of 3 is reached.
    pub async fn fill_event_triggers(&self, n: usize) {
        for i in 0..n {
            let tool = format!("evt_tool_{i}");
            self.ok("host.tool_publish", json!({"name": tool, "kind": "echo", "spec": {"schema": {"type": "object"}}})).await;
            self.ok(
                "host.trigger.set",
                json!({"tool": tool, "name": format!("evt_{i}"), "kind": "event", "verify": {"scheme": "none", "allow_unverified": true}}),
            )
            .await;
        }
    }
}

impl Fixture {
    pub async fn tenant(&self) -> mcphost::db::Tenant {
        self.server
            .state
            .db
            .find_tenant_by_namespace(self.namespace.clone())
            .await
            .expect("find tenant")
            .expect("tenant exists")
    }
}

/// A template a test parses itself (the shipped ones are in `templates/`).
pub fn fixture_template(name: &str, source: &str) -> mcphost::templates::Template {
    let mut t: mcphost::templates::Template = toml::from_str(source).expect("fixture template parses");
    t.name = name.to_string();
    t
}

/// Two loopback targets answering 200 (the `http` kind the fixture registers allows loopback).
pub async fn probe_targets() -> (Vec<wiremock::MockServer>, Vec<String>) {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let mut servers = Vec::new();
    let mut urls = Vec::new();
    for n in 1..=2 {
        let up = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).mount(&up).await;
        urls.push(format!("{}/probe-target-{n}", up.uri()));
        servers.push(up);
    }
    (servers, urls)
}
