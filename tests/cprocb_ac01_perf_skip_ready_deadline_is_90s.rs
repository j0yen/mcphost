//! PRD-mcphost-child-process-test-harness-b AC1 — under `MCPHOST_PERF_SKIP=1`
//! `ServeChild::wait_ready()` against a healthy child uses the 90 s deadline,
//! taken from one `serve_ready_timeout()` call with no literal seconds in
//! `tests/support/serve.rs`.

use crate::common;
use crate::serve;

use common::EnvGuard;
use serve::{ServeChild, ServeEnv};
use std::time::Duration;

#[tokio::test]
async fn perf_skip_makes_wait_ready_use_the_90s_deadline() {
    let _env = EnvGuard::set("MCPHOST_PERF_SKIP", "1").await;
    let data_dir = std::env::temp_dir().join(format!("mcphost-cprocb-ac01-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut server = ServeChild::spawn(ServeEnv {
        data_dir: data_dir.clone(),
        admin_key: Some("cprocb-ac01-admin-key".into()),
        secret_key: Some("cprocb-ac01-secret-key".into()),
        extra_env: vec![("MCPHOST_LOG_LEVEL".into(), "warn".into())],
        drain_stdout: false,
    });
    server.wait_ready().await;
    // The configured deadline compared with the advertised one, not a measurement.
    let widened_deadline = Duration::from_millis(90_000);
    assert_eq!(server.ready_budget(), Some(widened_deadline));
    assert_eq!(common::serve_ready_timeout(), widened_deadline);
    drop(server);
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[test]
fn serve_rs_reads_its_deadline_from_one_call_and_has_no_literal_seconds() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/serve.rs"),
    )
    .unwrap();
    let code: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect();
    let calls = code.iter().filter(|l| l.contains("serve_ready_timeout(")).count();
    assert_eq!(calls, 1, "exactly one serve_ready_timeout() call in serve.rs");
    assert!(
        !code.iter().any(|l| l.contains("from_secs") || l.contains("as_secs")),
        "serve.rs must carry no literal seconds"
    );
}
