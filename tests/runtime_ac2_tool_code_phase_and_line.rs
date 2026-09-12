//! AC2 (P0) — Given a python tool whose code raises ValueError at a known
//! line, When called, Then the result is a structured tool error with
//! phase `tool_code`, the exception class, and that line — no bare
//! traceback fragment.

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn value_error_at_a_known_line_names_phase_class_and_line() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // The `raise` is on line 3 -- pinned so the assertion below is exact,
    // not "some plausible-looking number".
    let source = "\
def main(args):
    n = args.get('n', 0)
    raise ValueError(f'n must be positive, got {n}')
";
    let spec = json!({"source": source, "args_schema": {"type": "object"}});
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "always_raises", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = poll_until_ready(
        &client,
        &format!("{ns}.always_raises"),
        json!({"n": -1}),
        Duration::from_secs(10),
    )
    .await
    .expect_err("main() always raises");

    assert_eq!(err.error_code.as_deref(), Some("tool_exception"));
    assert_eq!(
        err.data["phase"].as_str(),
        Some("tool_code"),
        "data: {:?}",
        err.data
    );
    assert_eq!(err.data["exception_class"].as_str(), Some("ValueError"));
    assert_eq!(err.data["line"].as_i64(), Some(3));
    // The message itself is a clean one-liner (class + text + location),
    // never the raw multi-line traceback blob standing in as the message
    // (that stays available separately in data.traceback for anyone who
    // wants it).
    assert!(!err.message.contains("Traceback (most recent call last)"));
    assert!(err.message.contains("ValueError"));
    assert!(err.message.contains("n must be positive"));
}
