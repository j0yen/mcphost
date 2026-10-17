//! PRD-mcphost-sandbox-file-processing-libs
//! AC3 (P0) -- Given a fresh data dir, When the daemon starts, Then
//! `envs/_base/<hash>/` exists within 120 s with all five libs importable
//! and `du -sm` <= 150.

use crate::common;
use mcphost::kinds::python::PythonKind;
use mcphost::sandbox_libs;
use std::time::Duration;

#[tokio::test]
async fn sblibs_ac03_base_env_exists_within_120s_with_all_libs_under_150mb() {
    let data_dir = common::TempDataDir::new();
    let kind = PythonKind::new(&data_dir.0);
    let expected = data_dir.0.join("envs").join("_base").join(sandbox_libs::lock_hash());
    assert_eq!(kind.base_env_dir(), expected, "base env lives at envs/_base/<lock-hash>");
    assert!(!expected.exists(), "a fresh data dir has no base env yet");

    // What `main.rs` does at daemon start.
    kind.ensure_base_env().await;
    let env_dir = kind
        .wait_base_env(Duration::from_secs(120))
        .await
        .expect("base env ready within 120 s");
    assert_eq!(env_dir, expected);
    assert!(env_dir.is_dir());

    let python = env_dir.join("bin").join("python");
    let imports: Vec<&str> = sandbox_libs::libs().iter().map(|l| l.import.as_str()).collect();
    assert_eq!(imports.len(), 5);
    let out = std::process::Command::new(&python)
        .arg("-c")
        .arg(format!("import {}", imports.join(", ")))
        .output()
        .expect("run base env python");
    assert!(
        out.status.success(),
        "all five libs must import: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let du = std::process::Command::new("du").arg("-sm").arg(&env_dir).output().expect("du");
    let mb: u64 = String::from_utf8_lossy(&du.stdout)
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .expect("du -sm prints a number");
    assert!(mb <= sandbox_libs::BASE_ENV_MAX_MB, "base env is {mb} MB, over the 150 MB cap");
}
