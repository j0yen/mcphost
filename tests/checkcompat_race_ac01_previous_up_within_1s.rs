//! PRD-mcphost-checkcompat-port-race AC1 (P0) — Given a previous binary of
//! `/bin/false`, When `check_compat` runs, Then it returns exit 4 naming
//! step `previous-up` within 1s and the log records the child's exit
//! status.
//!
//! Same "spawn `mcphost migrate --check-compat` as a real subprocess"
//! harness `tests/checkcompat_ac02_ac03.rs` uses (this file is
//! self-contained per the PRD's technical considerations, so it
//! duplicates rather than shares that helper). Requirement 3's short-
//! circuit (`Child::try_wait` polled every iteration, ahead of the old
//! 10s timeout) is what makes the 1s bound meetable at all: `/bin/false`
//! exits immediately without ever binding the loopback port, so the very
//! first poll iteration sees it gone.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

#[test]
fn broken_previous_binary_fails_fast_naming_previous_up() {
    let data_dir = scratch_data_dir("ac01");
    seed_live_db(&data_dir);
    let db_path = data_dir.join("mcphost.db");

    // Same broken-previous stand-in as ac02_ac03.rs's AC3 test: `false`
    // exits immediately without ever binding the loopback port.
    let broken_previous = if std::path::Path::new("/bin/false").exists() {
        "/bin/false"
    } else {
        "/usr/bin/false"
    };

    let start = Instant::now();
    let output = Command::new(mcphost_bin())
        .arg("migrate")
        .arg("--check-compat")
        .arg("--previous")
        .arg(broken_previous)
        .arg("--db")
        .arg(&db_path)
        .output()
        .expect("run mcphost migrate --check-compat");
    let elapsed = start.elapsed();

    assert_eq!(
        output.status.code(),
        Some(4),
        "check-compat should exit 4 on a broken previous binary, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        elapsed.as_secs_f64() < 1.0,
        "check-compat should fail within 1s of a previous binary that never comes up, took {elapsed:?}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("step 'previous-up'"),
        "stderr should name the failing step as 'previous-up', got: {stderr}"
    );

    // Requirement 3: "exit code 4 names the step and the child's exit
    // status" -- the child's exit status is recorded in the tracing log
    // (JSON on stdout, since `init_tracing()` runs before check-compat).
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("exit status"),
        "log should record the child's exit status, got stdout: {stdout}"
    );

    let _ = fs::remove_dir_all(&data_dir);
}
