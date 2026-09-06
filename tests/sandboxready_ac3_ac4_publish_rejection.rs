//! PRD-mcphost-sandbox-ready
//! AC3 -- Given `sandbox_ready: false`, When a tenant calls
//! `host.tool_publish`/`host.tool_test`/`host.tool_run` with a valid
//! python-kind spec, Then the response is `sandbox_unavailable`, states the
//! spec was not evaluated, and `data.mechanism`/`data.detail`/`data.docs`/
//! `data.alternatives == ["echo","http"]` are present, returned in under 1s.
//! AC4 -- Given `sandbox_ready: false`, When the same tenant publishes an
//! `http`-kind and an `echo`-kind tool, Then both succeed exactly as on a
//! ready host.

mod common;
use common::{fake_interpreter_failing, signup};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::{HttpKind, LookupFuture, NameLookup};
use mcphost::kinds::python::PythonKind;
use mcphost::sandbox;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct NoLookup;
impl NameLookup for NoLookup {
    fn lookup(&self, _host: String) -> LookupFuture {
        Box::pin(async {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no lookups needed by this test",
            ))
        })
    }
}

/// `echo` + `http` + a `python` kind whose sandbox self-test has already
/// been run and found unready -- the full registry `main.rs` builds in
/// production (AC4 needs `http` and `echo` both present alongside the
/// broken `python` kind).
async fn unready_registry(data_dir: &std::path::Path) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HttpKind::for_test(
        "127.0.0.1",
        Arc::new(NoLookup),
    )));
    let py = Arc::new(PythonKind::for_test_with_selftest(data_dir, 300));
    py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
        "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted",
    )));
    py.run_startup_selftest().await;
    kinds.register(py);
    kinds
}

fn assert_sandbox_unavailable_shape(err: &common::RpcError) {
    assert_eq!(err.error_code.as_deref(), Some("sandbox_unavailable"));
    assert!(
        err.message.to_lowercase().contains("not evaluated"),
        "message must state the spec was not evaluated, got: {}",
        err.message
    );
    assert!(
        err.data
            .get("mechanism")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()),
        "data.mechanism must be present: {}",
        err.data
    );
    assert!(
        err.data
            .get("detail")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()),
        "data.detail must be present: {}",
        err.data
    );
    assert!(
        err.data
            .get("docs")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()),
        "data.docs must be present: {}",
        err.data
    );
    assert_eq!(
        err.data.get("alternatives"),
        Some(&json!(["echo", "http"])),
        "data: {}",
        err.data
    );
}

#[tokio::test]
async fn tool_publish_is_rejected_fast_with_the_structured_shape() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let kinds = unready_registry(&data_dir.0).await;
    let server = common::TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let started = Instant::now();
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "my_py",
                "kind": "python",
                "spec": {"source": "def main(args):\n    return {}\n"},
            }),
        )
        .await
        .expect_err("publish on an unready sandbox must be rejected");
    let elapsed = started.elapsed();
    assert_sandbox_unavailable_shape(&err);
    assert!(
        elapsed < Duration::from_secs(1),
        "must reject in under 1s, took {elapsed:?}"
    );
}

#[tokio::test]
async fn tool_test_and_tool_run_reject_an_already_published_python_tool() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    // Publish the python tool while the sandbox is READY, then flip it to
    // unready -- proving the gate applies to an already-published tool's
    // host.tool_test/host.tool_run, not just a fresh publish.
    let data_dir = common::TempDataDir::new();
    let (kinds, py) = common::python_kind_registry_with_selftest(&data_dir.0, 300);
    py.run_startup_selftest().await;
    assert!(py.current_sandbox_status().ready, "must start ready");

    let server = common::TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "AC3 Tenant 2").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "my_py",
                "kind": "python",
                "spec": {"source": "def main(args):\n    return {}\n"},
            }),
        )
        .await
        .expect("publish while ready must succeed");

    py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
        "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted",
    )));
    py.recheck_sandbox().await;
    assert!(!py.current_sandbox_status().ready, "must now be unready");

    let err = client
        .tools_call("host.tool_test", json!({"name": "my_py", "args": {}}))
        .await
        .expect_err("host.tool_test must reject once the sandbox is unready");
    assert_sandbox_unavailable_shape(&err);

    let err = client
        .tools_call("host.tool_run", json!({"name": "my_py", "args": {}}))
        .await
        .expect_err("host.tool_run must reject once the sandbox is unready");
    assert_sandbox_unavailable_shape(&err);
}

#[tokio::test]
async fn echo_and_http_publish_unaffected_by_an_unready_python_sandbox() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let data_dir = common::TempDataDir::new();
    let kinds = unready_registry(&data_dir.0).await;
    let server = common::TestServer::start_with_kinds(kinds).await;
    let (ns, key) = signup(&server.base_url, "AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ec", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("echo publish must succeed exactly as on a ready host");
    client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "ht",
                "kind": "http",
                "spec": {
                    "method": "GET",
                    "url": "https://api.example.com/items/{{id}}",
                },
            }),
        )
        .await
        .expect("http publish must succeed exactly as on a ready host");

    let listed = client.tools_list().await.expect("tools/list");
    let names: Vec<String> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    assert!(names.contains(&format!("{ns}.ec")), "got: {names:?}");
    assert!(names.contains(&format!("{ns}.ht")), "got: {names:?}");
}
