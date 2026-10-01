//! PRD-mcphost-tool-naming-convention-and-aliases
//! AC2 (P0) -- Given a client calling `host.tool_share` [sic: any alias]
//! with valid arguments, When dispatched, Then the result equals a call
//! to its canonical with the same arguments, the response carries the
//! deprecation hint, and exactly one `tool_deprecated_alias` log line
//! appears for that tenant that day across 5 calls.
//!
//! Uses `host.tool_list`/`host.tool.list` (no side effects, so five
//! back-to-back calls are trivially idempotent) rather than
//! `host.tool_share` itself -- any of the 20 aliases exercises the same
//! dispatch-resolution/hint/log code path; this one needs no published
//! tool as a fixture.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::{Value, json};
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

/// `_meta` is the one field that may legitimately differ between an alias
/// call's result and its canonical's (the deprecation hint) -- strip it
/// before the equality check.
fn without_meta(mut v: Value) -> Value {
    if let Some(obj) = v.as_object_mut() {
        obj.remove("_meta");
    }
    v
}

#[tokio::test]
async fn alias_call_equals_canonical_carries_hint_and_logs_once_per_day() {
    let buf = BufWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(buf.clone())
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("install test tracing subscriber");

    let server = TestServer::start().await;
    let (tenant, key) = common::signup(&server.base_url, "alias-caller").await;
    let authed = McpClient::with_bearer(&server.base_url, &key);

    let canonical_result = authed.tools_call("host.tool.list", json!({})).await.expect("canonical call");

    let mut alias_results = Vec::new();
    for _ in 0..5 {
        let r = authed.tools_call("host.tool_list", json!({})).await.expect("alias call");
        alias_results.push(r);
    }

    for r in &alias_results {
        assert_eq!(
            without_meta(r.clone()),
            without_meta(canonical_result.clone()),
            "an alias call must equal a canonical call aside from _meta"
        );
        let meta = r.get("_meta").expect("alias call must carry _meta");
        let dep = meta.get("x-deprecated").expect("x-deprecated");
        assert_eq!(dep["replaced_by"], "host.tool.list");
        assert_eq!(dep["sunset"], mcphost::tool_aliases::ALIAS_SUNSET_DATE);
    }

    let captured = String::from_utf8(buf.0.lock().expect("lock buf").clone()).expect("utf8 log");
    let matching: Vec<&str> = captured
        .lines()
        .filter(|l| l.contains("tool_deprecated_alias") && l.contains(&tenant))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one tool_deprecated_alias line for this tenant across 5 alias calls: {captured}"
    );
}
