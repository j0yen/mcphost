//! PRD-mcphost-auth-error-names-argument
//! AC6 (P1) — Given a refused call, When the journal is read, Then the
//! `WARN` line contains the code and the tool name and no key material.
//!
//! Full journald capture isn't practical from an integration test, so this
//! captures the process's own tracing output instead (same pattern as
//! `tests/sessionkey_ac17_request_log_no_key.rs`), which is what actually
//! feeds journald under `journalctl -u mcphost` in production -- close
//! enough to the acceptance criterion's intent to exercise the real
//! `tracing::warn!` call sites in `src/handler.rs::call_tool`, not a
//! reimplementation of them.

use crate::common;
use common::{McpClient, TestServer};
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
async fn warn_line_carries_code_and_tool_but_never_the_key() {
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buf.clone())
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("install the test's global tracing subscriber");

    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    // Missing tenant_key on one tool, an unrecognized one on another --
    // exercise both new WARN sites.
    client
        .tools_call("host.tool_list", serde_json::json!({}))
        .await
        .expect_err("missing tenant_key must be refused");
    client
        .tools_call(
            "host.usage",
            serde_json::json!({"tenant_key": "t_super_secret_guess"}),
        )
        .await
        .expect_err("unrecognized tenant_key must be refused");

    let captured = String::from_utf8(buf.0.lock().expect("lock buf").clone())
        .expect("captured log is UTF-8");

    assert!(
        captured.contains("WARN"),
        "expected at least one WARN line: {captured}"
    );
    assert!(
        captured.contains("tenant_key_missing") && captured.contains("host.tool_list"),
        "WARN line for the missing-key call must carry its code and tool name: {captured}"
    );
    assert!(
        captured.contains("tenant_key_invalid") && captured.contains("host.usage"),
        "WARN line for the invalid-key call must carry its code and tool name: {captured}"
    );
    assert!(
        !captured.contains("t_super_secret_guess"),
        "no WARN line may carry the key material itself: {captured}"
    );
}
