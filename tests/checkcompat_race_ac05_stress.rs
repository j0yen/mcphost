//! PRD-mcphost-checkcompat-port-race AC5: 30 "dummy" tests, each spawning
//! a real previous `mcphost` on a fresh listener via `check_compat`'s
//! default (inherited-fd) path, meant to run alongside
//! `tests/checkcompat_ac02_ac03.rs`'s two tests under
//! `cargo test -- --test-threads=32` -- the exact concurrency level that
//! exposed the original `free_loopback_port` race (more cores, more
//! parallel threads racing the same ephemeral port pool). Looping the
//! whole suite 200 times at that thread count (the PRD's own stress
//! protocol) is a CI/operator action outside a single `cargo test`
//! invocation; this file is the fixture that protocol runs against.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch_data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-checkcompat-race-stress-{tag}-{}-{}",
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

fn run_one_dummy(tag: &str) {
    let data_dir = scratch_data_dir(tag);
    seed_live_db(&data_dir);
    let db_path = data_dir.join("mcphost.db");

    let output = Command::new(mcphost_bin())
        .arg("migrate")
        .arg("--check-compat")
        .arg("--previous")
        .arg(mcphost_bin()) // itself, as its own previous release
        .arg("--db")
        .arg(&db_path)
        .output()
        .expect("run mcphost migrate --check-compat");

    assert!(
        output.status.success(),
        "dummy {tag}: check-compat should exit 0 against a real previous binary, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&data_dir);
}

macro_rules! dummy_test {
    ($name:ident, $tag:literal) => {
        #[test]
        fn $name() {
            run_one_dummy($tag);
        }
    };
}

dummy_test!(checkcompat_race_dummy_01, "d01");
dummy_test!(checkcompat_race_dummy_02, "d02");
dummy_test!(checkcompat_race_dummy_03, "d03");
dummy_test!(checkcompat_race_dummy_04, "d04");
dummy_test!(checkcompat_race_dummy_05, "d05");
dummy_test!(checkcompat_race_dummy_06, "d06");
dummy_test!(checkcompat_race_dummy_07, "d07");
dummy_test!(checkcompat_race_dummy_08, "d08");
dummy_test!(checkcompat_race_dummy_09, "d09");
dummy_test!(checkcompat_race_dummy_10, "d10");
dummy_test!(checkcompat_race_dummy_11, "d11");
dummy_test!(checkcompat_race_dummy_12, "d12");
dummy_test!(checkcompat_race_dummy_13, "d13");
dummy_test!(checkcompat_race_dummy_14, "d14");
dummy_test!(checkcompat_race_dummy_15, "d15");
dummy_test!(checkcompat_race_dummy_16, "d16");
dummy_test!(checkcompat_race_dummy_17, "d17");
dummy_test!(checkcompat_race_dummy_18, "d18");
dummy_test!(checkcompat_race_dummy_19, "d19");
dummy_test!(checkcompat_race_dummy_20, "d20");
dummy_test!(checkcompat_race_dummy_21, "d21");
dummy_test!(checkcompat_race_dummy_22, "d22");
dummy_test!(checkcompat_race_dummy_23, "d23");
dummy_test!(checkcompat_race_dummy_24, "d24");
dummy_test!(checkcompat_race_dummy_25, "d25");
dummy_test!(checkcompat_race_dummy_26, "d26");
dummy_test!(checkcompat_race_dummy_27, "d27");
dummy_test!(checkcompat_race_dummy_28, "d28");
dummy_test!(checkcompat_race_dummy_29, "d29");
dummy_test!(checkcompat_race_dummy_30, "d30");
