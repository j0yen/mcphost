//! contract: ci-test-partition.sh check ages a pending measured entry in three rungs
//! (WARN past 3 on a main push, red at 6, red past 3 under `check --main`)
//! A land that seeds a `_provisional` entry used to turn main red four first-parent commits later unless
//! someone ran `scripts/ci-refresh-weights.sh <run id>` by hand, and a red main holds every land.
//! `PENDING_MAX_AGE` (3) now only fails `check --main`; on a main push (GITHUB_REF_NAME == CI_DEFAULT_BRANCH) age 4-5
//! prints a line starting `WARN pending-entry-age: ` and exits 0, and `PENDING_HARD_MAX_AGE` (6) fails the push too.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn pending_entry_age_rungs_warn_on_main_push_and_fail_at_the_hard_max() {
    // scripts/ci-partition-pending-age-selftest.sh builds ONE fixture repo under `mktemp -d` and proves: age 3 passes
    // on a push and under --main with no WARN; age 4 and 5 WARN on a push (rc 0) and fail --main; age 6 fails both;
    // a non-main ref at age 6 is byte-identical (rc and output) to origin/main's unmodified script.
    let o = Command::new("bash")
        .arg(root().join("scripts/ci-partition-pending-age-selftest.sh"))
        .env_remove("PENDING_AGE_GOLDEN")
        .current_dir(root())
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "{out}\n{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("selftest ok"), "{out}");
    assert!(!out.contains("FAIL"), "{out}");
}
