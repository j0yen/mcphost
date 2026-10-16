//! PRD-mcphost-graceful-drain-contract: the shared fixture of the
//! `gdrain_ac*` tests -- a real `mcphost serve` child with a python `slow`
//! tool published for N tenants (a free tenant admits 4 concurrent calls, so
//! the 20-call wave spreads over 5), plus readers for the `restarts` and
//! `calls` rows the child leaves behind.
//!
//! Not a crate module: pulled in with `#[path = "support/gdrain.rs"] mod gdrain;`.

#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::common::{self, McpClient, RpcError};
use crate::serve::{ServeChild, ServeEnv};

pub type CallResult = Result<Value, RpcError>;

pub struct Fixture {
    pub server: ServeChild,
    pub data_dir: PathBuf,
    keys: Vec<(String, String)>,
}

/// Callers check `mcphost::sandbox::require_user_namespaces_or_ci_skip()`
/// first (the python tool needs the sandbox).
pub async fn start(tag: &str, drain_s: u64, tenants: usize) -> Fixture {
    let data_dir = std::env::temp_dir().join(format!("mcphost-gdrain-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut server = ServeChild::spawn(ServeEnv {
        data_dir: data_dir.clone(),
        admin_key: Some("gdrain-admin-key".into()),
        secret_key: Some("gdrain-secret-key".into()),
        extra_env: vec![
            ("MCPHOST_LOG_LEVEL".into(), "warn".into()),
            ("MCPHOST_DRAIN_S".into(), drain_s.to_string()),
            ("MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR".into(), "100".into()),
        ],
        ..ServeEnv::default()
    });
    server.wait_ready().await;
    let base = server.base_url();
    let mut keys = Vec::new();
    for i in 0..tenants {
        let (ns, key) = common::signup(&base, &format!("gdrain {tag} {i}")).await;
        let client = McpClient::with_bearer(&base, &key);
        let spec = json!({
            "source": "import time\ndef main(args):\n    time.sleep(args.get(\"secs\", 0))\n    return {\"ok\": True, \"secs\": args.get(\"secs\", 0)}\n",
            "args_schema": {"type": "object"},
        });
        client
            .tools_call("host.tool_publish", json!({"name": "slow", "kind": "python", "spec": spec}))
            .await
            .expect("publish slow");
        // Warm the environment so the wave below is the real call path.
        common::poll_until_ready(&client, &format!("{ns}.slow"), json!({"secs": 0}), Duration::from_secs(30))
            .await
            .expect("warm slow");
        keys.push((ns, key));
    }
    Fixture { server, data_dir, keys }
}

impl Fixture {
    /// Start `n` concurrent `slow` calls of `secs` seconds, spread round-robin
    /// over the tenants; returns once each is on its way.
    pub async fn fire(&self, n: usize, secs: u64) -> Vec<tokio::task::JoinHandle<CallResult>> {
        let base = self.server.base_url();
        let mut handles = Vec::new();
        for i in 0..n {
            let (ns, key) = self.keys[i % self.keys.len()].clone();
            let client = McpClient::with_bearer(&base, &key);
            handles.push(tokio::spawn(async move {
                client.tools_call(&format!("{ns}.slow"), json!({"secs": secs})).await
            }));
        }
        // Let every request reach the handler before the caller signals.
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        handles
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.data_dir.join("mcphost.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open mcphost.db")
    }

    /// `(in_flight_at_signal, finished, dropped, drain_ms, bound_s)` of the
    /// only `restarts` row.
    pub fn restart_row(&self) -> (i64, i64, i64, i64, i64) {
        self.db()
            .query_row(
                "SELECT in_flight_at_signal, finished, dropped, drain_ms, bound_s FROM restarts",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .expect("exactly one restarts row")
    }

    pub fn restart_id(&self) -> String {
        self.db()
            .query_row("SELECT restart_id FROM restarts", [], |r| r.get(0))
            .expect("restart_id")
    }

    /// How many `slow` calls rows carry `error_class = class`.
    pub fn calls_with_class(&self, class: &str) -> i64 {
        self.db()
            .query_row(
                "SELECT COUNT(*) FROM calls WHERE tool_name = 'slow' AND error_class = ?1",
                [class],
                |r| r.get(0),
            )
            .expect("count calls")
    }
}
