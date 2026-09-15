//! PRD-mcphost-checkcompat-port-race AC1 + the "time for `/bin/false` case
//! to fail" success metric: requirement 3's `try_wait()` short-circuit
//! means a previous binary that exits immediately (never binds anything)
//! must fail in well under a second, not after the old 10s readiness
//! timeout. `tests/checkcompat_ac02_ac03.rs` already covers the exit
//! code/step-name half of this scenario (AC3 of the original migration-
//! safety PRD); this file is self-contained per this PRD's
//! `test_prefix: checkcompat_race` and adds the timing assertion only.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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

#[test]
fn check_compat_fails_fast_on_a_binary_that_exits_immediately() {
    let data_dir = scratch_data_dir("ac01");
    seed_live_db(&data_dir);
    let db_path = data_dir.join("mcphost.db");

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
        "expected exit 4, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("step 'spawn'"),
        "stderr should name the failing step, got: {stderr}"
    );
    assert!(
        elapsed.as_secs_f64() < 1.0,
        "AC1/success-metric: a previous binary that exits immediately should \
         fail in under 1s via try_wait(), took {elapsed:?}"
    );

    let _ = fs::remove_dir_all(&data_dir);
}
