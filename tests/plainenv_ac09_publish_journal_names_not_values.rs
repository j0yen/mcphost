//! PRD-mcphost-python-kind-plain-env AC9 (P1) — Given a publish with env,
//! When the journal/audit row is read, Then it records the env names and
//! never the values.
//!
//! Full journald capture isn't practical from an integration test, so this
//! captures the process's own tracing output instead -- same pattern as
//! tests/autherr_ac6_warn_journal_no_key.rs, which is what actually feeds
//! journald in production.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock buf").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for BufWriter {
    type Writer = BufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn publish_journal_line_names_env_keys_never_values() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buf.clone())
        .with_target(false)
        .finish();
    // Best-effort: if another test in this binary already installed a
    // global subscriber, this one silently loses the race rather than
    // panicking -- accept a possibly-empty capture below in that case
    // rather than making this test flaky under a consolidated test binary.
    let installed = tracing::subscriber::set_global_default(subscriber).is_ok();

    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    const SECRET_LOOKING_VALUE: &str = "should-never-be-journaled-8f3d";
    let spec = json!({
        "source": "def main(args):\n    return {}\n",
        "args_schema": {"type": "object"},
        "env": {"UPSTREAM_URL": SECRET_LOOKING_VALUE},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "journaled", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    if !installed {
        println!("skipping assertions: another test already owns the global tracing subscriber");
        return;
    }

    let captured = String::from_utf8(buf.0.lock().expect("lock buf").clone())
        .expect("captured log is UTF-8");
    assert!(
        captured.contains("UPSTREAM_URL"),
        "the publish journal line must name the env key: {captured}"
    );
    assert!(
        !captured.contains(SECRET_LOOKING_VALUE),
        "the publish journal must never carry an env value: {captured}"
    );
}
