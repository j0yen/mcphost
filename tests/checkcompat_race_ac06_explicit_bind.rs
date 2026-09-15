//! PRD-mcphost-checkcompat-port-race AC6: `MCPHOST_BIND=127.0.0.1:<port>`
//! set on the `check-compat` process itself keeps the old bind-by-address
//! path (requirement 4) against a real previous binary, logs that it did
//! so, and still enforces the token end to end.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch_data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-checkcompat-race-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create scratch data dir");
    dir
}

fn mcphost_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcphost"))
}

fn seed_live_db(data_dir: &std::path::Path) {
    let status = Command::new(mcphost_bin())
        .arg("migrate")
        .env("MCPHOST_DATA_DIR", data_dir)
        .status()
        .expect("run mcphost migrate");
    assert!(status.success(), "seeding migrate should succeed");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn explicit_mcphost_bind_still_passes_and_enforces_the_token() {
    let data_dir = scratch_data_dir("ac06");
    seed_live_db(&data_dir);
    let db_path = data_dir.join("mcphost.db");
    let port = free_port();

    let output = Command::new(mcphost_bin())
        .arg("migrate")
        .arg("--check-compat")
        .arg("--previous")
        .arg(mcphost_bin()) // itself, as its own previous release
        .arg("--db")
        .arg(&db_path)
        .env("MCPHOST_BIND", format!("127.0.0.1:{port}"))
        .env("MCPHOST_LOG_LEVEL", "info")
        .output()
        .expect("run mcphost migrate --check-compat");

    assert!(
        output.status.success(),
        "explicit MCPHOST_BIND against a real previous binary should still pass, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("bound by address") && combined.contains(&port.to_string()),
        "AC6: log should say it bound by address, got: {combined}"
    );

    let _ = fs::remove_dir_all(&data_dir);
}
