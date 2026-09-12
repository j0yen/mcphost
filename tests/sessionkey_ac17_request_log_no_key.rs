//! PRD-mcphost-session-key
//! AC17 — Given a call carrying `tenant_key`, When the server writes its
//! structured request log line, Then no substring of the key appears in
//! that line.

use crate::common;
use common::{McpClient, TestServer, signup};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// Same pattern as `compat_ac13_ac14_middleware_logging_and_scope.rs`'s
/// `BufWriter`: capture every `tracing::info!` line the process emits into
/// an in-memory buffer. This must be the only test in this binary that
/// installs a global subscriber.
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
async fn structured_request_log_never_carries_a_tenant_key_argument() {
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buf.clone())
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("install the test's global tracing subscriber");

    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Logged Tenant").await;

    // Every call from here on carries tenant_key as an argument, with no
    // Authorization header at all.
    let client = McpClient::new(&server.base_url);
    client
        .tools_call("host.whoami", serde_json::json!({"tenant_key": key}))
        .await
        .expect("whoami over tenant_key");
    client
        .tools_call(
            "host.tool_publish",
            serde_json::json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object"}},
                "tenant_key": key,
            }),
        )
        .await
        .expect("publish over tenant_key");

    let captured = String::from_utf8(buf.0.lock().expect("lock buf").clone())
        .expect("captured log is UTF-8");
    assert!(
        !captured.contains(&key),
        "the structured request log must never contain the tenant_key value: {captured}"
    );
}
