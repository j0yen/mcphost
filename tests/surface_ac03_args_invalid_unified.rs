//! PRD-mcphost-surface-fluidity AC3 — Given a `host.tool_call` with an
//! argument of the wrong type, When it runs, Then the error code is
//! `args_invalid`; and `grep -r invalid_args src/ docs/ www/` returns
//! nothing.
//!
//! Requirement 2: `invalid_args` is removed as a wire code -- every site
//! that used to emit it (the missing-control-plane-argument path, e.g.
//! `host.tool_publish` with no `name`) now emits `args_invalid` too, the
//! same code the `args_schema` pre-check already used.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;
use std::fs;
use std::path::Path;

/// The literal grep AC3 pins: no file under `src/`, `docs/`, or `www/` may
/// contain the substring `invalid_args` (case-sensitive -- this is
/// deliberately not `InvalidArgs`, the still-live Rust enum variant name,
/// which AC3's own grep would not match either).
#[test]
fn grep_invalid_args_returns_nothing_under_src_docs_www() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut offenders = Vec::new();
    for dir in ["src", "docs", "www"] {
        walk(&root.join(dir), &mut offenders);
    }
    assert!(
        offenders.is_empty(),
        "grep -r invalid_args src/ docs/ www/ must return nothing, found in: {offenders:?}"
    );
}

fn walk(dir: &Path, offenders: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, offenders);
        } else if let Ok(contents) = fs::read_to_string(&path)
            && contents.contains("invalid_args")
        {
            offenders.push(path.display().to_string());
        }
    }
}

#[tokio::test]
async fn missing_control_plane_argument_now_reports_args_invalid_too() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Surface AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // `host.tool_publish` with no `name` at all used to hit
    // `AppError::InvalidArgs`, wire code `invalid_args`. Requirement 2: one
    // code for bad arguments -- this is now `args_invalid` too.
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"kind": "echo", "spec": {"type": "object"}}),
        )
        .await
        .expect_err("a tool_publish with no name must fail");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
}

#[tokio::test]
async fn host_tool_call_with_wrong_typed_argument_reports_args_invalid() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Surface AC3 Tenant 2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // The echo kind's spec wraps the JSON Schema under `schema` (see
    // `kinds/echo.rs`'s `schema_of`) -- passing the schema bare, as an
    // earlier version of this test did, fails publish itself with
    // `invalid_spec` before ever reaching the args-type check this test
    // means to exercise.
    let spec = json!({
        "schema": {
            "type": "object",
            "properties": {"n": {"type": "integer"}},
            "required": ["n"],
        },
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "needs_int", "kind": "echo", "spec": spec}),
        )
        .await
        .expect("publish ok");

    // `n` is a string, not the required integer -- routed through
    // `host.tool_call` per AC3's own wording, rather than the namespaced
    // form (same dispatch path either way).
    let err = client
        .tools_call(
            "host.tool_call",
            json!({"name": "needs_int", "args": {"n": "not-an-integer"}}),
        )
        .await
        .expect_err("a wrong-typed argument must fail");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"));
}
