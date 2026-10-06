//! PR "compat-check: chown the scratch dir and db copy to the run-as uid
//! before spawning the previous release": `chown_scratch_for` is the seam
//! -- chowning to the current process's own uid/gid is always permitted
//! without root, so these tests drive it directly via
//! `compat_check::test_support`, the same seam
//! `tests/checkcompat_race_ac*.rs` and `tests/compatfix_ac1_*.rs` already
//! use, rather than requiring a privileged test runner.

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-compatchown-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

#[test]
fn ac01_scratch_and_copied_files_are_owned_by_run_as() {
    let dir = scratch_dir("ac01");
    std::fs::write(dir.join("mcphost.db"), b"fake db").expect("write fake mcphost.db");
    std::fs::write(dir.join("mcphost.db-wal"), b"fake wal").expect("write fake mcphost.db-wal");

    // SAFETY: getuid()/getgid() take no arguments and cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };

    // Chown to self: always permitted without root, so this exercises the
    // real chown syscall (not just the None-skip branch) in a hermetic test.
    mcphost::compat_check::test_support::chown_scratch_for(Some((uid, gid)), &dir)
        .expect("chown to self must succeed");

    let dir_meta = std::fs::metadata(&dir).expect("stat scratch dir");
    assert_eq!(dir_meta.uid(), uid, "scratch dir uid");
    assert_eq!(dir_meta.gid(), gid, "scratch dir gid");
    for name in ["mcphost.db", "mcphost.db-wal"] {
        let meta = std::fs::metadata(dir.join(name)).expect("stat copied file");
        assert_eq!(meta.uid(), uid, "{name} uid");
        assert_eq!(meta.gid(), gid, "{name} gid");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ac02_skipped_with_no_filesystem_access_when_run_as_none() {
    // A path that does not exist: if the `None` branch touched the
    // filesystem at all (listing or chowning), this would error.
    let never_touched = std::path::Path::new("/nonexistent/compatchown-must-not-touch");
    mcphost::compat_check::test_support::chown_scratch_for(None, never_touched)
        .expect("run_as None must be a pure no-op, not even a stat");
}
