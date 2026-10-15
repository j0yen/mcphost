//! PRD-mcphost-task-handoff-live-script AC1 (P0) — Given a checkout of the
//! repo, When `bash scripts/task-handoff-live.sh` runs with no argument, Then
//! it prints usage on stderr and exits 2; and `bash -n` exits 0 and the
//! script references no command other than bash builtins, `curl` and
//! `python3`.

use std::path::PathBuf;
use std::process::Command;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/task-handoff-live.sh")
}

#[test]
fn no_argument_prints_usage_on_stderr_and_exits_2() {
    let out = Command::new("bash").arg(script()).output().expect("run script");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(out.stdout.is_empty(), "usage must not touch stdout: {out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("usage:"), "stderr: {err}");
}

#[test]
fn the_script_parses_and_names_no_command_beyond_builtins_curl_and_python3() {
    let syntax = Command::new("bash").arg("-n").arg(script()).output().expect("bash -n");
    assert_eq!(syntax.status.code(), Some(0), "{syntax:?}");

    let src = std::fs::read_to_string(script()).expect("read script");
    // External commands a shell script commonly reaches for; none may appear
    // in command position (line start, after `|`, `;`, `&&`, `||`, `$(`).
    let banned = [
        "sed", "awk", "grep", "egrep", "cat", "date", "sleep", "head", "tail", "tr", "cut", "jq",
        "mktemp", "wc", "sort", "uniq", "xargs", "env", "bc", "expr", "timeout", "nc", "wget",
        "perl", "ruby", "node", "python", "ssh", "rm", "mv", "cp", "tee", "basename", "dirname",
    ];
    for (n, line) in src.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        for sep in ["", "| ", "|| ", "&& ", "; ", "$(", "`"] {
            for b in banned {
                let needle = format!("{sep}{b} ");
                let hit = if sep.is_empty() { t.starts_with(&needle) } else { t.contains(&needle) };
                assert!(!hit, "line {}: external command `{b}` in `{line}`", n + 1);
            }
        }
    }
    assert!(src.starts_with("#!/usr/bin/env bash\n"), "shebang");
    assert!(src.contains("set -euo pipefail"), "strict mode");
}
