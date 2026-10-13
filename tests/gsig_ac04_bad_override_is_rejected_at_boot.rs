//! PRD-mcphost-golden-signals-self-probe AC4 (P0): `MCPHOST_HEAL_TOML`
//! pointing at a file with `[signals.echo_p95_ms] red = "fast"` makes the host
//! log `heal.toml override rejected` at boot and keep serving the compiled
//! `red = 2000`.
//!
//! Spawns the real `mcphost serve` (the override is read from the process
//! environment at boot, which an in-process server in this shared test binary
//! must not mutate).

use std::time::{Duration, Instant};

use crate::serve;
use serve::{ServeChild, ServeEnv};

#[tokio::test]
async fn bad_override_logs_rejection_and_status_keeps_compiled_red() {
    let dir = std::env::temp_dir().join(format!("gsig-ac04-{}", std::process::id()));
    let data_dir = dir.join("data");
    std::fs::create_dir_all(&data_dir).expect("create scratch dirs");
    let heal = dir.join("heal.toml");
    std::fs::write(&heal, "[signals.echo_p95_ms]\nred = \"fast\"\n").expect("write override");

    let mut server = ServeChild::spawn(ServeEnv {
        data_dir,
        admin_key: Some("gsig-ac04-admin-key".into()),
        secret_key: Some("gsig-ac04-secret-key".into()),
        extra_env: vec![("MCPHOST_HEAL_TOML".into(), heal.display().to_string())],
        drain_stdout: true,
    });
    server.wait_ready().await;

    let deadline = Instant::now() + Duration::from_secs(5);
    while !server.stdout_log().contains("heal.toml override rejected") && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let log = server.stdout_log();
    assert!(log.contains("heal.toml override rejected"), "boot log:\n{log}");

    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url()))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse body");
    assert_eq!(body["signals"]["echo_p95_ms"]["red"], 2000, "body: {body}");

    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}
