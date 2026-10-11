//! Shared fixture for the `uprg_ac*` tests (PRD-mcphost-uptime-probe-recipe-green):
//! a server with the relaxed `http` kind (loopback + plain http), a
//! signed-up tenant, and fixture targets -- two wiremock servers answering
//! 200 at `.../probe-target-<n>` and one URL whose port refuses connections.

use crate::common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

pub struct Fixture {
    pub _server: TestServer,
    pub client: McpClient,
    pub urls: Vec<String>,
    _upstreams: Vec<MockServer>,
}

/// A URL on a loopback port nothing listens on.
pub fn refusing_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}/probe-target-3")
}

pub async fn ok_upstream(n: usize) -> (MockServer, String) {
    let upstream = MockServer::start().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).mount(&upstream).await;
    let url = format!("{}/probe-target-{n}", upstream.uri());
    (upstream, url)
}

/// Starts the server and a free tenant, with the three AC1 fixture URLs.
pub async fn start() -> Fixture {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_namespace, key) = signup(&server.base_url, "uprg tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let (u1, url1) = ok_upstream(1).await;
    let (u2, url2) = ok_upstream(2).await;
    Fixture { _server: server, client, urls: vec![url1, url2, refusing_url()], _upstreams: vec![u1, u2] }
}

impl Fixture {
    pub async fn create(&self, args: Value) -> Result<Value, crate::common::RpcError> {
        self.client.tools_call("host.uptime.create", args).await.map(|r| extract_structured(&r))
    }

    pub async fn create_status(&self) -> Value {
        self.create(json!({"name": "status", "urls": self.urls}))
            .await
            .unwrap_or_else(|e| panic!("host.uptime.create failed: {} {}", e.code, e.message))
    }
}

/// `host.tool_call(name)`'s own result object, whichever envelope it rides in.
pub fn call_result(v: &Value) -> Value {
    let v = extract_structured(v);
    for key in ["result", "payload"] {
        if v.get(key).is_some_and(|r| r.get("rows").is_some()) {
            return v[key].clone();
        }
    }
    v
}

/// What exists for the tenant: (tool names, trigger count, table names).
pub async fn inventory(client: &McpClient) -> (Vec<String>, usize, Vec<String>) {
    let tools = extract_structured(&client.tools_call("host.tool_list", json!({})).await.expect("tool_list"));
    let tools = tools["tools"].as_array().expect("tools").iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
    let triggers = extract_structured(&client.tools_call("host.trigger.list", json!({})).await.expect("trigger.list"));
    let n = triggers["triggers"].as_array().expect("triggers").len();
    let tables = extract_structured(&client.tools_call("host.table.list", json!({})).await.expect("table.list"));
    let tables = tables["tables"].as_array().map(|a| a.iter().map(|t| t["name"].as_str().unwrap_or_default().to_string()).collect()).unwrap_or_default();
    (tools, n, tables)
}
