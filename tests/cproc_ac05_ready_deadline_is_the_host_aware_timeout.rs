//! PRD-mcphost-child-process-test-harness AC5 — under `MCPHOST_PERF_SKIP=1`,
//! `ServeChild::wait_ready()` against a healthy child uses the 90 s deadline
//! `serve_ready_timeout()` returns, and `serve.rs` holds exactly one call to
//! it and no literal seconds.

use crate::common::EnvGuard;
use std::time::Duration;

use crate::serve;
use serve::{ServeChild, ServeEnv};

#[tokio::test]
async fn perf_skip_widens_the_ready_deadline_to_90s() {
    let _env = EnvGuard::set("MCPHOST_PERF_SKIP", "1").await;
    let data_dir = std::env::temp_dir().join(format!("mcphost-cproc-ac05-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut server = ServeChild::spawn(ServeEnv {
        data_dir: data_dir.clone(),
        admin_key: Some("cproc-ac05-admin-key".into()),
        secret_key: Some("cproc-ac05-secret-key".into()),
        extra_env: vec![("MCPHOST_LOG_LEVEL".into(), "warn".into())],
        drain_stdout: false,
    });
    assert_eq!(server.ready_budget(), None);
    server.wait_ready().await;
    assert_eq!(server.ready_budget(), Some(Duration::from_millis(90_000)));
    drop(server);
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[test]
fn serve_rs_has_one_serve_ready_timeout_call_and_no_literal_seconds() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/serve.rs"),
    )
    .unwrap();
    let code: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
    let calls = code.iter().filter(|l| l.contains("serve_ready_timeout(")).count();
    assert_eq!(calls, 1, "serve.rs must read the deadline from serve_ready_timeout() exactly once");
    assert!(
        !code.iter().any(|l| l.contains("from_secs") || l.contains("as_secs")),
        "serve.rs must carry no literal seconds"
    );
}
