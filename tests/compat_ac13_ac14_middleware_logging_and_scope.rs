//! AC13 — Given a request to `POST /mcp` with
//! `MCP-Protocol-Version: 2026-07-28` and no SEP-2243 headers, When rmcp
//! refuses it, Then the server logs one structured request line recording
//! the non-200 status, so the operator can see the refusal in `journalctl`
//! without a packet capture.
//! AC14 — Given `GET /healthz`, When it is called, Then the documented
//! scope of the `MCP-Protocol-Version` layer in `src/http.rs` matches the
//! observed behaviour -- either the header is absent because the layer
//! was scoped to `/mcp`, or the doc comment states that the layer is
//! router-wide.

mod common;
use common::TestServer;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// A `MakeWriter` that appends every write into a shared in-memory buffer,
/// so a test can install it as the process's tracing subscriber and then
/// read back whatever `tracing::info!` lines the server emitted while the
/// subscriber was active.
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

/// AC13 sets a JSON-formatted global tracing subscriber writing into an
/// in-memory buffer (matching `src/main.rs`'s production `.json()`
/// format), issues one refused request, and parses the captured lines
/// back as JSON to find the request-line log entry `protocol_version_and_log`
/// emits unconditionally (before or after refusal -- the middleware wraps
/// the whole handler chain, so it runs regardless of the response status).
///
/// This must be the only test in the binary that installs a global
/// subscriber (`tracing::subscriber::set_global_default` panics if called
/// twice in one process), so this file keeps it to this single test.
#[tokio::test]
async fn ac13_refused_request_still_gets_one_structured_log_line() {
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buf.clone())
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("install the test's global tracing subscriber");

    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    // MCP-Protocol-Version >= STANDARD_HEADERS with no Mcp-Method header:
    // rmcp's own SEP-2243 validation refuses this before mcphost's handler
    // runs (this is exactly the shape defect B used to cause every real
    // client to send).
    let resp = http
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {},
        }))
        .send()
        .await
        .expect("send request");
    let status = resp.status();
    assert!(
        !status.is_success(),
        "a 2026-07-28 declaration with no SEP-2243 headers must be refused: {status}"
    );

    let captured = String::from_utf8(buf.0.lock().expect("lock buf").clone())
        .expect("captured log is UTF-8");
    let refusal_line = captured
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|v| v.get("fields").and_then(|f| f.get("path")).and_then(|p| p.as_str()) == Some("/mcp"));
    let refusal_line = refusal_line.unwrap_or_else(|| {
        panic!("no structured \"http request\" log line for /mcp found in captured output: {captured}")
    });
    let fields = &refusal_line["fields"];
    assert_eq!(
        fields["status"].as_u64(),
        Some(status.as_u16() as u64),
        "the log line must record the actual non-200 status: {refusal_line}"
    );
    assert_eq!(fields["method"].as_str(), Some("POST"));
}

#[tokio::test]
async fn ac14_healthz_header_presence_matches_the_module_doc_comment() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .get(format!("{}/healthz", server.base_url))
        .send()
        .await
        .expect("send healthz");
    let header_present = resp.headers().contains_key("MCP-Protocol-Version");

    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/http.rs"))
        .expect("read src/http.rs");
    let doc_says_router_wide =
        (src.contains("attached to the") && src.contains("whole router"))
            || src.contains("is router-wide");

    if header_present {
        assert!(
            doc_says_router_wide,
            "GET /healthz carries MCP-Protocol-Version, so the middleware's doc comment \
             in src/http.rs must say the layer is router-wide, not scoped to /mcp"
        );
    } else {
        // The other option AC14 accepts: the layer was scoped to /mcp, so
        // /healthz legitimately has no header at all.
    }
}
