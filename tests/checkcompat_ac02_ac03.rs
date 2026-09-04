//! PRD-mcphost-migration-safety AC2/AC3: `mcphost migrate --check-compat`
//! end to end, spawning the *real* `mcphost` binary as a subprocess (via
//! `CARGO_BIN_EXE_mcphost`, cargo's own path to the just-built binary --
//! see https://doc.rust-lang.org/cargo/reference/environment-variables.html).
//!
//! AC2 (success path) uses the current binary as its own "previous"
//! release: since it is exactly the schema it just migrated to, the check
//! must pass, prove the live database file is byte-for-byte untouched, and
//! leave no scratch directory behind.
//!
//! AC3 (failure path) needs a "previous" binary that cannot serve the
//! migrated schema. This repo has no second compiled release to spawn, so
//! the deliberately-broken stand-in is `--previous` pointed at a binary
//! that can never come up as `mcphost serve` would (here, the system
//! `false`, which exits immediately without binding a port) -- exactly the
//! shape of "the previous release can't run against the migrated schema"
//! the check exists to catch, exercised without needing two real releases
//! on hand. The readiness probe times out and the check must exit 4 naming
//! the `spawn` step, without ever touching the live database.

use std::fs;
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

/// Run `mcphost migrate` (no --check-compat) once against `data_dir` to
/// create and fully migrate a live database, the way `mcphost serve` would
/// on first start.
fn seed_live_db(data_dir: &std::path::Path) {
    let status = Command::new(mcphost_bin())
        .arg("migrate")
        .env("MCPHOST_DATA_DIR", data_dir)
        .status()
        .expect("run mcphost migrate");
    assert!(status.success(), "seeding migrate should succeed");
}

#[test]
fn check_compat_passes_against_a_compatible_previous_binary_and_leaves_live_db_untouched() {
    let data_dir = scratch_data_dir("ac02");
    seed_live_db(&data_dir);
    let db_path = data_dir.join("mcphost.db");
    let before = fs::read(&db_path).expect("read live db before check");

    let child = Command::new(mcphost_bin())
        .arg("migrate")
        .arg("--check-compat")
        .arg("--previous")
        .arg(mcphost_bin()) // stand-in "previous" release: itself
        .arg("--db")
        .arg(&db_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn mcphost migrate --check-compat");
    let child_pid = child.id();
    let output = child
        .wait_with_output()
        .expect("wait for mcphost migrate --check-compat");

    assert!(
        output.status.success(),
        "check-compat should exit 0, got {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let after = fs::read(&db_path).expect("read live db after check");
    assert_eq!(before, after, "the live database must be byte-for-byte untouched");

    // The check-compat process's own scratch dir (named after its own PID,
    // see src/compat_check.rs's ScratchDir) must not remain -- matched by
    // that exact child PID so a concurrently-running sibling test's
    // still-in-flight scratch dir under the same shared temp dir is never
    // mistaken for a leftover of this one.
    let prefix = format!("mcphost-checkcompat-{child_pid}-");
    let leftovers: Vec<_> = fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .collect();
    assert!(
        leftovers.is_empty(),
        "scratch dir(s) left behind: {leftovers:?}"
    );

    let _ = fs::remove_dir_all(&data_dir);
}

#[test]
fn check_compat_fails_naming_the_step_when_the_previous_binary_cannot_come_up() {
    let data_dir = scratch_data_dir("ac03");
    seed_live_db(&data_dir);
    let db_path = data_dir.join("mcphost.db");
    let before = fs::read(&db_path).expect("read live db before check");

    // `/bin/false` (or `/usr/bin/false`) stands in for a previous release
    // that cannot serve the migrated schema: it exits immediately without
    // ever binding the loopback port, so the readiness probe times out.
    let broken_previous = if std::path::Path::new("/bin/false").exists() {
        "/bin/false"
    } else {
        "/usr/bin/false"
    };

    let output = Command::new(mcphost_bin())
        .arg("migrate")
        .arg("--check-compat")
        .arg("--previous")
        .arg(broken_previous)
        .arg("--db")
        .arg(&db_path)
        .output()
        .expect("run mcphost migrate --check-compat");

    assert_eq!(
        output.status.code(),
        Some(4),
        "check-compat should exit 4 on a broken previous binary, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("step 'spawn'"),
        "stderr should name the failing step, got: {stderr}"
    );

    let after = fs::read(&db_path).expect("read live db after check");
    assert_eq!(
        before, after,
        "the live database must be untouched even when the check fails"
    );

    let _ = fs::remove_dir_all(&data_dir);
}
