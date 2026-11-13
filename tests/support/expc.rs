#![allow(dead_code)]
//! Shared fixture for the `expc_ac*` tests (PRD-mcphost-export-data-class-
//! completeness-v2): a tenant with messages, calls, a secret and blobs; run
//! `host.export` to completion and read the archive back as `path -> bytes`.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::{Duration, Instant};

use base64::Engine as _;
use flate2::read::GzDecoder;
use serde_json::{Value, json};
use tar::Archive;

use crate::common::{McpClient, TestServer, extract_structured, signup};

pub const SECRET_VALUE: &str = "expc-secret-value-5d1f";

pub struct Tenant {
    pub ns: String,
    pub client: McpClient,
}

pub async fn new_tenant(server: &TestServer, name: &str) -> Tenant {
    let (ns, key) = signup(&server.base_url, name).await;
    Tenant { ns, client: McpClient::with_bearer(&server.base_url, &key) }
}

pub fn read_archive(path: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read archive {path:?}: {e}"));
    let mut out = BTreeMap::new();
    for entry in Archive::new(GzDecoder::new(&bytes[..])).entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let name = entry.path().expect("entry path").to_string_lossy().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read entry");
        out.insert(name, data);
    }
    out
}

pub async fn wait_for_done(client: &McpClient, run_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let got = extract_structured(
            &client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("host.runs.get ok"),
        );
        if got["status"] == json!("done") || got["status"] == json!("error") {
            return got;
        }
        assert!(Instant::now() < deadline, "run never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub fn lines(data: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(data)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("jsonl line {l:?}: {e}")))
        .collect()
}

impl Tenant {
    pub async fn call(&self, tool: &str, args: Value) -> Value {
        extract_structured(
            &self
                .client
                .tools_call(tool, args)
                .await
                .unwrap_or_else(|e| panic!("{tool} failed: {} {}", e.code, e.message)),
        )
    }

    /// `threads` threads of `per_thread` messages each, sent to this tenant by
    /// `sender` (so they land in this tenant's inbox, which `threads.jsonl` carries).
    pub async fn receive_threads(&self, sender: &Tenant, threads: usize, per_thread: usize) {
        for t in 0..threads {
            let first = sender
                .call("host.msg.send", json!({"to": [self.ns.clone()], "body": format!("thread {t} message 0")}))
                .await;
            let thread_id = first["thread_id"].as_str().expect("thread_id").to_string();
            for m in 1..per_thread {
                sender
                    .call(
                        "host.msg.send",
                        json!({"to": [self.ns.clone()], "thread_id": thread_id, "body": format!("thread {t} message {m}")}),
                    )
                    .await;
            }
        }
    }

    /// One echo tool called `n` times: `n` rows in `calls`.
    pub async fn make_calls(&self, n: usize) {
        self.call("host.tool_publish", json!({"name": "ping", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
            .await;
        for i in 0..n {
            self.call(&format!("{}.ping", self.ns), json!({"i": i})).await;
        }
    }

    pub async fn put_blob(&self, name: &str, bytes: &[u8]) {
        self.call(
            "host.blob.put",
            json!({
                "name": name,
                "mime": "application/octet-stream",
                "content_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await;
    }

    /// Runs `host.export {args}` to completion; returns `(run_id, entries)`.
    pub async fn export(&self, server: &TestServer, args: Value) -> (String, BTreeMap<String, Vec<u8>>) {
        let enq = self.call("host.export", args).await;
        let run_id = enq["run_id"].as_str().unwrap_or_else(|| panic!("no run_id: {enq}")).to_string();
        let done = wait_for_done(&self.client, &run_id).await;
        assert_eq!(done["status"], json!("done"), "export failed: {done:?}");
        let path = server.state.db.data_dir().join("exports").join(format!("{run_id}.tar.gz"));
        (run_id, read_archive(&path))
    }
}
