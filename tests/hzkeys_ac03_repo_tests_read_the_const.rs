//! PRD-mcphost-healthz-anon-keys-const AC3 (P0) -- at the landing commit
//! `scripts/healthz-keys-lint.py tests` exits 0 over the real tests tree (no
//! test restates the anonymous key set). The four named tests
//! (`hzver_ac01`, `ctrv_ac03`, `busyaudit_ac08`, `ac14_storage_unwritable`)
//! read `common::anon_healthz_keys()` and run in the core suites.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn real_tests_tree_has_no_literal_anonymous_key_set() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("python3")
        .arg("scripts/healthz-keys-lint.py")
        .arg("tests")
        .current_dir(&root)
        .output()
        .expect("run healthz-keys-lint.py");
    assert_eq!(
        out.status.code(),
        Some(0),
        "tests restate the anonymous key set:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_named_tests_read_the_common_helper() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    for f in [
        "hzver_ac01_anonymous_healthz_keys.rs",
        "ctrv_ac03_status_and_healthz_carry_contract_identity.rs",
        "busyaudit_ac08_healthz_db_fields.rs",
        "ac14_storage_unwritable.rs",
    ] {
        let src = std::fs::read_to_string(root.join(f)).expect(f);
        assert!(src.contains("anon_healthz_keys()") || src.contains("assert_anon_healthz("), "{f} must read the const through common");
    }
}
