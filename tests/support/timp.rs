#![allow(dead_code)]
//! Shared fixture for the `timp_ac*` tests (PRD-mcphost-export-tables-and-
//! import-restore): seed a tenant with a table, a table note, a doc, a tool,
//! state and a secret; run `host.export` / `host.import` to completion; read
//! an archive back as `path -> bytes`.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use serde_json::{Value, json};
use tar::Archive;

use crate::common::{McpClient, TestServer, extract_structured, signup};

pub fn read_archive_entries(path: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read archive {path:?}: {e}"));
    let mut archive = Archive::new(GzDecoder::new(&bytes[..]));
    let mut out = BTreeMap::new();
    for entry in archive.entries().expect("tar entries") {
        let mut entry = entry.expect("tar entry");
        let entry_path = entry.path().expect("entry path").to_string_lossy().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).expect("read entry data");
        out.insert(entry_path, data);
    }
    out
}

pub async fn wait_for_done(client: &McpClient, run_id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let got = extract_structured(
            &client
                .tools_call("host.runs.get", json!({"run_id": run_id}))
                .await
                .expect("host.runs.get ok"),
        );
        if got["status"] == json!("done") || got["status"] == json!("error") {
            return got;
        }
        assert!(Instant::now() < deadline, "run never finished: {got:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub struct Tenant {
    pub client: McpClient,
}

pub async fn new_tenant(server: &TestServer, name: &str) -> Tenant {
    let (_ns, key) = signup(&server.base_url, name).await;
    Tenant { client: McpClient::with_bearer(&server.base_url, &key) }
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

    /// Table `t` (id integer pk, name text) with `rows` rows and a table note.
    pub async fn seed_table(&self, rows: usize) {
        self.call(
            "host.table.create",
            json!({"name": "t", "columns": {"id": "integer", "name": "text"}, "primary_key": "id"}),
        )
        .await;
        let rows: Vec<Value> = (1..=rows).map(|i| json!({"id": i, "name": format!("row {i}")})).collect();
        for chunk in rows.chunks(200) {
            self.call("host.table.append", json!({"table": "t", "rows": chunk})).await;
        }
        self.call(
            "host.table.model_set",
            json!({"table": "t", "key": "description", "value": "the t table"}),
        )
        .await;
    }

    pub async fn seed_doc(&self, name: &str, content: &str) {
        self.call("host.docs.put", json!({"name": name, "content": content})).await;
    }

    /// Table, doc, state key and secret: the fixture tenant the ACs share.
    pub async fn seed_full(&self) {
        self.seed_table(3).await;
        self.seed_doc("a.md", "# a doc").await;
        self.call("host.state.set", json!({"key": "k1", "value": {"n": 1}})).await;
        self.call("host.secret_set", json!({"name": "api_key", "value": "sekrit"})).await;
        self.call(
            "host.tool_publish",
            json!({"name": "xx", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await;
    }

    /// Runs `host.export` to completion; returns `(run_id, entries)`.
    pub async fn export(&self, server: &TestServer) -> (String, BTreeMap<String, Vec<u8>>) {
        let enq = self.call("host.export", json!({})).await;
        let run_id = enq["run_id"].as_str().expect("run_id").to_string();
        let done = wait_for_done(&self.client, &run_id).await;
        assert_eq!(done["status"], json!("done"), "export failed: {done:?}");
        let path = server.state.db.data_dir().join("exports").join(format!("{run_id}.tar.gz"));
        (run_id, read_archive_entries(&path))
    }
}

/// Drops keys whose value is a clock reading or a host-assigned id, which
/// legitimately differ between two tenants holding the same data.
pub fn without_volatile(mut v: Value) -> Value {
    const VOLATILE: &[&str] = &[
        "computed_at", "updated_unix", "updated_at", "created_at", "created_unix", "id", "seq", "version",
        "bytes_used",
    ];
    match &mut v {
        Value::Object(map) => {
            map.retain(|k, _| !VOLATILE.contains(&k.as_str()));
            for child in map.values_mut() {
                *child = without_volatile(child.take());
            }
        }
        Value::Array(items) => {
            for child in items.iter_mut() {
                *child = without_volatile(child.take());
            }
        }
        _ => {}
    }
    v
}

impl Tenant {
    /// `host.state.list` minus the host's own `runs/…` result and progress
    /// keys (which name this tenant's own runs) and the quota block.
    pub async fn user_state(&self) -> Value {
        let listed = self.call("host.state.list", json!({})).await;
        let keys: Vec<Value> = listed["keys"]
            .as_array()
            .expect("keys array")
            .iter()
            .filter(|k| !k["key"].as_str().is_some_and(|k| k.starts_with("runs/")))
            .cloned()
            .collect();
        json!(keys)
    }

    /// The read-side views AC3 compares between two tenants.
    pub async fn views(&self) -> Value {
        json!({
            "table_list": without_volatile(self.call("host.table.list", json!({})).await),
            "table_describe": without_volatile(self.call("host.table.describe", json!({"table": "t"})).await),
            "state_list": self.user_state().await,
            "docs_list": without_volatile(self.call("host.docs.list", json!({})).await),
            "tool_list": without_volatile(self.call("host.tool_list", json!({})).await),
        })
    }

    /// `host.import(run_id=…)` to completion; returns the finished run.
    pub async fn import_run(&self, run_id: &str) -> Value {
        let enq = self.call("host.import", json!({"run_id": run_id})).await;
        let id = enq["run_id"].as_str().unwrap_or_else(|| panic!("no run_id: {enq}")).to_string();
        wait_for_done(&self.client, &id).await
    }
}
