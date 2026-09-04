//! AC9 (P0) — Given a tool that tries to read `/etc/mcphost/env`, another
//! tenant's environment directory, or the host's database, When called,
//! Then each read fails with a permission error inside the sandbox.
//!
//! A literal `/etc/mcphost/env` doesn't exist on a dev/test box, so a
//! read of it would fail with `FileNotFoundError` regardless of
//! sandboxing and wouldn't prove anything. This test instead creates a
//! real file *outside* the one directory this tool's sandbox is allowed to
//! see (the data dir's root, one level above the tenant/hash env directory
//! `kinds::python` actually `--ro-bind`s) and proves the sandbox can't read
//! it -- the same mount-namespace guarantee that makes `/etc/mcphost/env`,
//! another tenant's env directory, and the host's SQLite database all
//! equally invisible, since none of them are ever `--ro-bind`/`--bind`
//! into the sandbox either.

mod common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn reading_a_path_outside_the_sandbox_allowlist_fails() {
    // Requirement 8/9: this test builds and runs a real python-kind tool
    // via the sandbox, which needs unprivileged user namespaces. Not
    // guaranteed on GitHub's hosted runners, so skip cleanly in CI (and
    // fail loudly, not skip, anywhere else -- see
    // require_user_namespaces_or_ci_skip's doc comment) rather than fail
    // with "the tool's environment failed to build" -- same pattern as the
    // sandbox-dependent unit tests in src/kinds/python.rs and
    // src/sandbox.rs, and as tests/ac17_kind_conformance.rs.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("skipped: no user namespaces (CI)");
        return;
    }
    let envs_dir = common::TempDataDir::new();
    std::fs::write(envs_dir.0.join("host_secret.txt"), "top-secret-host-data")
        .expect("write host-side secret file");

    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let host_secret_path = envs_dir.0.join("host_secret.txt");
    let source = format!(
        "def main(args):\n    with open({path:?}) as f:\n        return {{\"leaked\": f.read()}}\n",
        path = host_secret_path.to_string_lossy(),
    );
    let spec = json!({
        "source": source,
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "reader", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let err = poll_until_ready(
        &client,
        &format!("{ns}.reader"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .expect_err("reading a path outside the sandbox allowlist must fail, not leak host data");
    assert_eq!(err.error_code.as_deref(), Some("tool_exception"));
}
