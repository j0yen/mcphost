//! PRD-mcphost-checkcompat-port-race AC6 (P1) — Given
//! `MCPHOST_BIND=127.0.0.1:<explicit port>`, When `check_compat` runs
//! against a real previous binary, Then it binds by address (log says so)
//! and still enforces the token.
//!
//! Same real-subprocess harness as `tests/checkcompat_ac02_ac03.rs`'s
//! success path, but with `$MCPHOST_BIND` set in check-compat's own env
//! (requirement 4: a caller that sets it explicitly keeps the old
//! bind-by-address path, unchanged, rather than the inherited-fd path).
//! `check_compat`'s own tracing (JSON on stdout, since `init_tracing()`
//! runs before it) is asserted directly rather than re-deriving "did it
//! bind by address" from the exit code alone, since exit 0 alone can't
//! distinguish "bound by address" from "used the inherited-fd path
//! anyway".

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch_data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-livedb-test-{tag}-{}-{}",
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

/// A momentarily-free port -- the explicit-bind path is, by requirement
/// 4's own design, the pre-existing "pick a port, hope for the best"
/// contract check-compat used everywhere before this PRD; it is not this
/// test's job to make that path itself race-free (goal 2 only covers the
/// default, inherited-fd path).
fn a_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

#[test]
fn explicit_mcphost_bind_binds_by_address_and_still_checks_the_token() {
    let data_dir = scratch_data_dir("ac06");
    seed_live_db(&data_dir);
    let db_path = data_dir.join("mcphost.db");

    let port = a_free_port();

    let output = Command::new(mcphost_bin())
        .arg("migrate")
        .arg("--check-compat")
        .arg("--previous")
        .arg(mcphost_bin()) // stand-in "previous" release: itself, as ac02_ac03.rs does
        .arg("--db")
        .arg(&db_path)
        .env("MCPHOST_BIND", format!("127.0.0.1:{port}"))
        .output()
        .expect("spawn mcphost migrate --check-compat");

    assert!(
        output.status.success(),
        "check-compat should exit 0 against a compatible previous binary, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("binding previous release by address"),
        "log should say it bound by address, got stdout: {stdout}"
    );
    assert!(
        stdout.contains(&format!("127.0.0.1:{port}")),
        "log should name the explicit address, got stdout: {stdout}"
    );
    // The token check still ran (and passed) on this path -- requirement
    // 2's `wait_ready` success message only fires once healthz answered
    // with the matching token.
    assert!(
        stdout.contains("answered healthz with our token"),
        "log should show the token check ran and passed, got stdout: {stdout}"
    );

    let _ = fs::remove_dir_all(&data_dir);
}
