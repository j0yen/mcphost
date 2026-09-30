//! PRD-mcphost-session-bound-tenant-after-signup
//! AC10 (P1) — Given any `tools/call` that `oauth_401_upgrade` rewrites to
//! 401, When the request-log line for it is read, Then its `status` field
//! is `401`, and for an unmodified 200 response it is `200`.
//!
//! Before requirement 7's fix, `src/http.rs`'s two middleware layers ran in
//! an order where `protocol_version_and_log` saw the response BEFORE
//! `oauth_401_upgrade` rewrote its status -- a `tenant_key_missing` refusal
//! (JSON-RPC 200 on the wire at that point, later rewritten to HTTP 401)
//! was journaled as `status: 200`. Layer order is now reversed so the log
//! line records the status actually sent to the client.
//!
//! Uses a thread-local (`tracing::subscriber::set_default`), not a global
//! subscriber (`compat_ac13_ac14_middleware_logging_and_scope.rs`'s own
//! doc comment: a global one may be installed only once per process, and
//! this repo's suite-consolidation bundles many test files into one
//! binary) -- safe here because `#[tokio::test]` defaults to the
//! current-thread runtime, so the whole request round trip below runs on
//! this same thread.
//!
//! `tracing::callsite::rebuild_interest_cache()` after `set_default` is
//! required, not optional, in this consolidated (~1000-test) binary:
//! `tracing`'s per-callsite `Interest` is cached globally the first time
//! any dispatcher is asked about it, and `set_default` alone does not
//! invalidate that cache. If some other test earlier in this same binary
//! exercises the `/mcp` request-logging callsite with no subscriber
//! installed at all (the ambient no-op default), the callsite can get
//! cached as "nobody's interested" for the rest of the process --
//! `set_default` here would then install a real subscriber that never
//! actually receives the event. `rebuild_interest_cache` forces every
//! callsite to re-register against whatever dispatcher (this test's
//! thread-local one) is active right now, independent of test order or
//! how many other `sessbind_ac*` files this PRD adds to the binary.
//!
//! That rebuild is necessary but, on its own, not sufficient: rebuilding is
//! a no-op for a call site nobody has registered yet, and this binary is
//! large enough that an unrelated concurrent `/mcp` request can be the one
//! to register it first, from a thread with no subscriber.
//! [`warm_up_request_log_callsite`] closes that gap -- see its own doc
//! comment, and `tests/support/busyaudit.rs`, which already does the same
//! thing for `db.rs`'s audit call sites. A global subscriber would close it
//! too, but `scripts/gen-test-suites.sh` gives every file containing
//! `set_global_default` its own singleton suite binary, and this repo is at
//! its ten-binary cap.

use crate::common;
use common::TestServer;
use serde_json::Value;
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

/// `src/http.rs`'s `"http request"` `tracing::info!` call site is hit by
/// every `/mcp` request any test in this ~1000-test binary makes, almost
/// all of them on threads with no subscriber installed. `tracing`'s
/// per-callsite interest cache is process-global and memoized on first
/// registration, and -- the part that matters here --
/// `rebuild_interest_cache()` is a no-op for a call site nobody has hit
/// yet. So installing the subscriber and rebuilding is not enough on its
/// own: if this test's rebuild runs before any `/mcp` request has ever
/// happened in the process, an unrelated concurrent request can register
/// the call site first from its own no-subscriber thread, caching it
/// "never interested" for good -- and this test's own log line is then
/// dropped before the thread-local dispatcher is ever consulted
/// (`last_mcp_status` reads `None`, for reasons having nothing to do with
/// the status being logged).
///
/// One throwaway `/mcp` request here, before the subscriber is installed,
/// guarantees the call site is already registered by the time the rebuild
/// runs -- exactly the pattern (and for exactly the reason)
/// `tests/support/busyaudit.rs::warm_up_db_audit_callsites` already uses
/// for `db.rs`'s `db_audit` call sites.
async fn warm_up_request_log_callsite(base_url: &str) {
    let _ = reqwest::Client::new()
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 0, "method": "ping", "params": {}}))
        .send()
        .await;
}

/// The `fields.status` an `/mcp` "http request" log line recorded for the
/// most recent matching call, read back out of `buf`.
fn last_mcp_status(buf: &BufWriter) -> Option<u64> {
    let captured = String::from_utf8(buf.0.lock().expect("lock buf").clone()).expect("captured log is UTF-8");
    captured
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v.get("fields").and_then(|f| f.get("path")).and_then(|p| p.as_str()) == Some("/mcp"))
        .filter_map(|v| v["fields"]["status"].as_u64())
        .next_back()
}

#[tokio::test]
async fn refused_call_logs_401_and_ok_call_logs_200() {
    // The server first, and one throwaway request through it, so the
    // request-log call site is registered before the subscriber below asks
    // `tracing` to recompute interest for it.
    let server = TestServer::start().await;
    warm_up_request_log_callsite(&server.base_url).await;

    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt().json().with_writer(buf.clone()).with_target(false).finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();

    let http = reqwest::Client::new();

    // A key-less, header-less host.* call: refused tenant_key_missing at
    // the JSON-RPC level, then rewritten to a real HTTP 401 by
    // `oauth_401_upgrade`.
    let refused = http
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "host.catalog.search")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "host.catalog.search",
                "arguments": {},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }))
        .send()
        .await
        .expect("send refused request");
    assert_eq!(refused.status(), reqwest::StatusCode::UNAUTHORIZED, "must be upgraded to a real 401");
    assert_eq!(
        last_mcp_status(&buf),
        Some(401),
        "the request-log line for the refused call must record 401, not the pre-upgrade 200"
    );

    // signup: unauthenticated, unaffected by oauth_401_upgrade, plain 200.
    let ok = http
        .post(format!("{}/mcp", server.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "signup")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "signup",
                "arguments": {"name": "AC10 Tenant"},
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            },
        }))
        .send()
        .await
        .expect("send signup request");
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
    assert_eq!(
        last_mcp_status(&buf),
        Some(200),
        "the request-log line for an unmodified success must still record 200"
    );
}
