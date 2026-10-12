//! PRD-mcphost-child-process-test-harness AC1 — a test that builds
//! `Command::new(bin).arg("serve")…stderr(Stdio::piped()).spawn()` and later
//! `read_to_string`s `child.stderr.take()` fails the lint naming both lines;
//! the same test rewritten over `ServeChild::spawn` + `diagnostics()` passes.

use crate::child_proc_lint;

use child_proc_lint::{fixture, run, scratch, write};

#[test]
fn raw_serve_spawn_and_live_pipe_read_exit_1_naming_both_lines() {
    let dir = scratch("ac01-bad");
    write(&dir, "hand_rolled_serve.rs", &fixture("hand_rolled_serve.rs.tpl"));
    let (code, out) = run(&dir);
    assert_eq!(code, 1, "lint output:\n{out}");
    // Line 5 = `Command::new` of the serve spawn chain, line 11 = the pipe read.
    assert!(out.contains("hand_rolled_serve.rs:5:"), "spawn line not named:\n{out}");
    assert!(out.contains("hand_rolled_serve.rs:11:"), "pipe-read line not named:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_same_test_over_serve_child_exits_0() {
    let dir = scratch("ac01-good");
    write(&dir, "over_helper.rs", &fixture("over_helper.rs.tpl"));
    let (code, out) = run(&dir);
    assert_eq!(code, 0, "lint output:\n{out}");
    assert!(out.contains("child-proc-lint: 0 offenders"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}
