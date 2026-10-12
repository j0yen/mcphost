//! PRD-mcphost-child-process-test-harness AC2 — a scratch file declaring the
//! old hand-rolled kill-guard struct fails the lint naming it; a
//! `Command::new(bin).arg("--version").output()` call passes (a waited child
//! is not a live pipe).

use crate::child_proc_lint;

use child_proc_lint::{fixture, run, scratch, write};

#[test]
fn guard_struct_exits_1_naming_it() {
    let dir = scratch("ac02-guard");
    write(&dir, "guard_struct.rs", &fixture("guard_struct.rs.tpl"));
    let (code, out) = run(&dir);
    assert_eq!(code, 1, "lint output:\n{out}");
    // Line 2 of the fixture declares the struct.
    assert!(out.contains("guard_struct.rs:2:"), "struct line not named:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_waited_version_call_exits_0() {
    let dir = scratch("ac02-waited");
    write(&dir, "waited_version.rs", &fixture("waited_version.rs.tpl"));
    let (code, out) = run(&dir);
    assert_eq!(code, 0, "lint output:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}
