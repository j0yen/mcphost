//! Flake lint: no wall-clock assertion under `tests/` may sit outside a
//! sanctioned timing helper (`perf_budget!`, `perf_overhead_budget!`,
//! `deadline_budget!`, ...), otherwise it fails under load and the test gets
//! quarantined as a baseline flake. PRD-mcphost-timing-assertion-lint: the
//! hand-typed `PATTERNS`/`ALLOW` lists are gone -- this runs the one scanner,
//! `scripts/timing-lint.py`, whose allow set is read from `tests/common/mod.rs`.

use std::path::Path;
use std::process::Command;

#[test]
fn every_timing_comparison_test_file_uses_perf_budget() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("python3")
        .arg("-I")
        .arg(root.join("scripts/timing-lint.py"))
        .arg("tests")
        .current_dir(root)
        .output()
        .expect("run scripts/timing-lint.py tests");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "wall-clock assertions outside a sanctioned timing helper (wrap in \
         crate::perf_budget!(ms, {{ .. }}) or crate::deadline_budget!(ms, {{ .. }})):\n{stdout}"
    );
}
