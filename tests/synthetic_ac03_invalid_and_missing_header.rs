//! PRD-mcphost-synthetic-flag
//! AC3 — Given signup requests with no header, an empty header, and an
//! invalid header (`Bad Label!`), When each completes, Then all three
//! tenants store null, all three signups succeed, and the invalid case
//! logs a warning.

use crate::common;
use common::{McpClient, TestServer, signup};
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// Same pattern as `sessionkey_ac17_request_log_no_key.rs`'s `BufWriter`:
/// capture every `tracing::warn!`/`info!` line into an in-memory buffer.
/// This must be the only test in this binary that installs a global
/// subscriber.
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
async fn absent_empty_and_invalid_headers_all_store_null_and_succeed() {
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buf.clone())
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("install the test's global tracing subscriber");

    let server = TestServer::start().await;

    // No header at all.
    let (ns_absent, _) = signup(&server.base_url, "No Header Agent").await;

    // Empty header value.
    let client = McpClient::new(&server.base_url);
    let empty_result = client
        .tools_call_with_header(
            "signup",
            serde_json::json!({"name": "Empty Header Agent"}),
            ("x-mcphost-synthetic", ""),
        )
        .await
        .expect("signup with empty header must succeed");
    let ns_empty = common::extract_structured(&empty_result)["tenant"]
        .as_str()
        .expect("tenant field")
        .to_string();

    // Invalid header value (space and punctuation both fail the charset).
    let invalid_result = client
        .tools_call_with_header(
            "signup",
            serde_json::json!({"name": "Invalid Header Agent"}),
            ("x-mcphost-synthetic", "Bad Label!"),
        )
        .await
        .expect("signup with invalid header must still succeed");
    let ns_invalid = common::extract_structured(&invalid_result)["tenant"]
        .as_str()
        .expect("tenant field")
        .to_string();

    // PRD-mcphost-tenant-attribution requirement 1 / AC1: all three come
    // from this suite's loopback test server with no valid explicit
    // stamp, so all three derive `source_class = loopback` and default
    // `synthetic` to `harness:unstamped` -- `None` is reserved for a
    // genuinely `external` tenant now, not "no valid header".
    for ns in [&ns_absent, &ns_empty, &ns_invalid] {
        let tenant = server
            .state
            .db
            .find_tenant_by_namespace(ns.clone())
            .await
            .expect("query")
            .unwrap_or_else(|| panic!("{ns} must exist"));
        assert_eq!(
            tenant.synthetic.as_deref(),
            Some("harness:unstamped"),
            "{ns} must default to harness:unstamped (got {:?})",
            tenant.synthetic
        );
    }

    let captured = String::from_utf8(buf.0.lock().expect("lock buf").clone())
        .expect("captured log is UTF-8");
    assert!(
        captured.contains("WARN") && captured.contains("Bad Label!"),
        "the invalid header must log a warning naming the rejected label: {captured}"
    );
}
