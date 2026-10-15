//! PRD-mcphost-task-handoff-live-script AC6 (P1) — Given `--json`, When the
//! script succeeds, Then stdout is one JSON object with `status`,
//! `latency_ms`, `task_id`, `thread_id` and nothing else.

use crate::common;
use crate::serve;
use serve::{ServeChild, ServeEnv};

use serde_json::Value;

#[tokio::test]
async fn json_flag_prints_one_object_with_exactly_four_keys() {
    let data = common::TempDataDir::new();
    let mut server = ServeChild::spawn(ServeEnv {
        data_dir: data.0.clone(),
        admin_key: Some("thlive6-admin".into()),
        secret_key: Some("thlive6-secret".into()),
        extra_env: vec![("MCPHOST_LOG_LEVEL".into(), "warn".into())],
        ..ServeEnv::default()
    });
    server.wait_ready().await;

    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/task-handoff-live.sh");
    let out = tokio::process::Command::new("bash")
        .arg(script)
        .arg("--json")
        .arg(server.base_url())
        .output()
        .await
        .expect("run script");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout.lines().count(), 1, "one line: {stdout:?}");

    let v: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("json {e}: {stdout:?}"));
    let obj = v.as_object().expect("object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["latency_ms", "status", "task_id", "thread_id"], "{stdout:?}");
    assert_eq!(obj["status"], "done");
    assert!(obj["latency_ms"].is_u64(), "{stdout:?}");
    assert!(obj["task_id"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(obj["thread_id"].as_str().is_some_and(|s| !s.is_empty()));
}
