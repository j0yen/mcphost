//! Shared fixture for the `piir_ac*` tests (PRD-mcphost-pii-tagging-and-log-redaction-v2):
//! a python-capable server with one tenant, a publisher for a tool that
//! prints and raises on its `email` argument, and a read of every persisted
//! sink of tool output straight from the database file.

#![allow(dead_code)]

use crate::common::{McpClient, RpcError, TempDataDir, TestServer, extract_structured, http_kind_registry, poll_until_ready, python_kind_registry, signup};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The tool: prints its `email` to stdout and stderr, then raises on it.
pub const LEAKY_SOURCE: &str = "import sys\ndef main(args):\n    print('email=' + args['email'])\n    print('err ' + args['email'], file=sys.stderr)\n    raise ValueError('bad email ' + args['email'])\n";

pub struct Fixture {
    pub server: TestServer,
    pub client: McpClient,
    pub ns: String,
    _envs: TempDataDir,
}

pub async fn start() -> Fixture {
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "piir tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    Fixture { server, client, ns, _envs: envs }
}

/// [`start`] with the http kind only: no sandbox needed.
pub async fn start_http() -> Fixture {
    let envs = TempDataDir::new();
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "piir http tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    Fixture { server, client, ns, _envs: envs }
}

/// Every persisted sink of tool output, by the `LogSink` display name.
#[derive(Debug, Default)]
pub struct Sinks {
    pub tool_log: Vec<String>,
    pub call_trace: Vec<String>,
    pub call_error_summary: Vec<String>,
    pub run_error: Vec<String>,
    pub message: Vec<String>,
}

impl Sinks {
    /// The sinks whose stored text contains `needle`.
    pub fn holding(&self, needle: &str) -> Vec<&'static str> {
        let mut held = Vec::new();
        for (name, rows) in self.named() {
            if rows.iter().any(|r| r.contains(needle)) {
                held.push(name);
            }
        }
        held
    }

    pub fn named(&self) -> [(&'static str, &Vec<String>); 5] {
        [
            ("ToolLog", &self.tool_log),
            ("CallTrace", &self.call_trace),
            ("CallErrorSummary", &self.call_error_summary),
            ("RunError", &self.run_error),
            ("Message", &self.message),
        ]
    }

    pub fn text_of(&self, sink: &str) -> String {
        self.named().iter().find(|(n, _)| *n == sink).map(|(_, rows)| rows.join("\n")).unwrap_or_default()
    }
}

impl Fixture {
    pub async fn publish(&self, name: &str, source: &str, pii: Option<Value>) -> Result<Value, RpcError> {
        let mut args = json!({
            "name": name,
            "kind": "python",
            "spec": {"source": source, "args_schema": {"type": "object", "properties": {"email": {"type": "string"}}}},
        });
        if let Some(pii) = pii {
            args["pii"] = pii;
        }
        self.client.tools_call("host.tool_publish", args).await.map(|v| extract_structured(&v))
    }

    /// Publishes an http tool POSTing its arguments to `url`, with `email` in its schema.
    pub async fn publish_http(&self, name: &str, url: &str, pii: Option<Value>) -> Result<Value, RpcError> {
        let mut args = json!({
            "name": name,
            "kind": "http",
            "spec": {
                "method": "POST",
                "url": url,
                "body": {"email": "{{ email }}"},
                "args_schema": {"type": "object", "properties": {"email": {"type": "string"}}},
            },
        });
        if let Some(pii) = pii {
            args["pii"] = pii;
        }
        self.client.tools_call("host.tool_publish", args).await.map(|v| extract_structured(&v))
    }

    /// Calls until the python environment is built, then returns the call
    /// that matters: the last answer (an error for the leaky tool).
    pub async fn warm(&self, name: &str) {
        let _ = poll_until_ready(&self.client, &format!("{}.{name}", self.ns), json!({"email": "warm@up.io"}), Duration::from_secs(90)).await;
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<Value, RpcError> {
        self.client
            .tools_call("host.tool_call", json!({"name": name, "args": args}))
            .await
            .map(|v| extract_structured(&v))
    }

    /// An async run, waited to a terminal state; returns `host.runs.get`.
    pub async fn run_async(&self, name: &str, args: Value) -> Value {
        let queued = extract_structured(
            &self
                .client
                .tools_call("host.tool_call", json!({"name": name, "args": args, "async": true}))
                .await
                .expect("enqueue"),
        );
        let run_id = queued["run_id"].as_str().expect("run_id").to_string();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let got = extract_structured(&self.client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("runs.get"));
            let status = got["status"].as_str().unwrap_or("");
            if matches!(status, "done" | "error" | "timeout" | "cancelled") || Instant::now() >= deadline {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Reads every sink from the database file.
    pub fn sinks(&self) -> Sinks {
        let conn = rusqlite::Connection::open(self.server.state.db.path()).expect("open db");
        let col = |sql: &str| -> Vec<String> {
            let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
            stmt.query_map([], |r| r.get::<_, String>(0)).expect("query").collect::<Result<Vec<_>, _>>().expect("rows")
        };
        Sinks {
            tool_log: col("SELECT line FROM logs"),
            call_trace: col(
                "SELECT request_json || char(10) || stdout_tail || char(10) || stderr_tail || char(10) || COALESCE(error_json, '') FROM call_traces",
            ),
            call_error_summary: col("SELECT error_summary FROM calls WHERE error_summary IS NOT NULL"),
            run_error: col("SELECT error_data_json FROM runs WHERE error_data_json IS NOT NULL"),
            message: col("SELECT body || char(10) || COALESCE(data_json, '') FROM messages"),
        }
    }

    pub async fn call_trace(&self, call_id: &str) -> Result<Value, RpcError> {
        self.client
            .tools_call("host.call_trace", json!({"call_id": call_id}))
            .await
            .map(|v| extract_structured(&v))
    }
}
