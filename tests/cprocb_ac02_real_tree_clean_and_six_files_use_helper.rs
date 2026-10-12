//! PRD-mcphost-child-process-test-harness-b AC2 — at the landing commit
//! `scripts/child-proc-lint.py tests` exits 0, no test declares its own child
//! guard struct, and the six former serve-spawning tests go through
//! `ServeChild` (`busyaudit_ac02` reading its server log via `stdout_log()`).
//! The six tests themselves run as ordinary members of the suite.

use crate::child_proc_lint;

use child_proc_lint::{repo_root, run_on};
use std::fs;

const SIX: [&str; 6] = [
    "ac11_load_smoke.rs",
    "ac12_preflight.rs",
    "agentdir_ac08_lookup_latency.rs",
    "msg_ac12_send_latency.rs",
    "busyaudit_ac02_env_override_busy_timeout.rs",
    "owncopy_ac02_from_and_reply_to_passed_verbatim.rs",
];

#[test]
fn lint_exits_zero_on_the_real_tests_tree() {
    let (code, out) = run_on(&repo_root().join("tests"));
    assert_eq!(code, 0, "child-proc-lint must pass on the real tree:\n{out}");
    assert!(out.contains("0 offenders"), "{out}");
}

#[test]
fn no_file_under_tests_declares_a_child_guard_struct() {
    // Built at run time so this file does not contain the needle itself.
    let needle = ["struct Child", "Guard"].concat();
    let out = std::process::Command::new("git")
        .args(["grep", "-c", &needle, "--", "tests/"])
        .current_dir(repo_root())
        .output()
        .expect("run git grep");
    // `git grep -c` prints nothing and exits 1 when there is no match.
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(out.stdout.is_empty());
}

#[test]
fn the_six_former_spawners_use_serve_child() {
    for name in SIX {
        let src = fs::read_to_string(repo_root().join("tests").join(name))
            .unwrap_or_else(|e| panic!("read {name}: {e}"));
        assert!(src.contains("ServeChild"), "{name} must spawn through ServeChild");
    }
}

#[test]
fn busyaudit_ac02_reads_its_server_log_via_stdout_log() {
    let src = fs::read_to_string(
        repo_root().join("tests/busyaudit_ac02_env_override_busy_timeout.rs"),
    )
    .expect("read busyaudit_ac02");
    assert!(src.contains("drain_stdout: true"), "must request the drain");
    assert!(src.contains(".stdout_log()"), "must read the log via stdout_log()");
}
