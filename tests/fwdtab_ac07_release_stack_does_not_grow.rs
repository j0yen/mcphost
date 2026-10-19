//! PRD-mcphost-generated-dispatch-table-stack-budget
//! AC7 (P1) — Given `cargo test --release` of the guard binary, When
//! `scripts/fwd-stack-measure.sh --release test:host.trigger.set
//! call:host.uptime.create call:host.table.chart` runs on `origin/main` (with
//! #413) and on the branch, Then each id's release-profile minimum stack on the
//! branch is <= its value on main and all six numbers appear in the PR body.
//!
//! A release build of the whole crate is minutes of compile, so the default run
//! checks the script's interface and the recorded main numbers; the measurement
//! itself runs with `FWDTAB_AC07_RELEASE=1 cargo test --test suite_core_07
//! fwdtab_ac07 -- --nocapture` (it builds the guard binary in release and
//! binary-searches the three ids).

use std::process::Command;

/// Release-profile minimum stack, in bytes, on `origin/main` d66b8328 (with #413),
/// measured by `scripts/fwd-stack-measure.sh --release` on 2026-10-17 (4 KiB steps).
const MAIN_RELEASE_MIN: &[(&str, usize)] = &[
    ("test:host.trigger.set", 266_240),
    ("call:host.uptime.create", 266_240),
    ("call:host.table.chart", 266_240),
];

fn script(args: &[&str]) -> std::process::Output {
    Command::new("bash")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("scripts/fwd-stack-measure.sh")
        .args(args)
        .output()
        .expect("run fwd-stack-measure.sh")
}

#[test]
fn fwdtab_ac07_script_takes_release_and_ids_and_refuses_anything_else() {
    let bad = script(&["--release"]);
    assert_eq!(bad.status.code(), Some(2), "no ids and no `all` is a usage error");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("usage:"), "{bad:?}");
    let bad = script(&["--release", "not-an-id"]);
    assert_eq!(bad.status.code(), Some(2), "{bad:?}");
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/fwd-stack-measure.sh")).expect("script");
    assert!(source.contains("--release") && source.contains("MCPHOST_FWD_STACK_BYTES"), "wraps the guard's knobs");
}

#[test]
fn fwdtab_ac07_release_minimum_on_the_branch_is_at_most_main() {
    if std::env::var("FWDTAB_AC07_RELEASE").ok().as_deref() != Some("1") {
        eprintln!("fwdtab_ac07: release measurement skipped (set FWDTAB_AC07_RELEASE=1 to build and measure)");
        return;
    }
    let ids: Vec<&str> = MAIN_RELEASE_MIN.iter().map(|(id, _)| *id).collect();
    let mut args = vec!["--release"];
    args.extend(ids.iter().copied());
    let out = script(&args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "fwd-stack-measure.sh --release failed: {stdout} {}", String::from_utf8_lossy(&out.stderr));
    for (id, main) in MAIN_RELEASE_MIN {
        let branch: usize = stdout
            .lines()
            .find_map(|l| l.strip_prefix(&format!("min {id} ")))
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no `min {id}` line in: {stdout}"));
        println!("{id}: main {main} B, branch {branch} B");
        assert!(branch <= *main, "{id} needs {branch} B on the branch > {main} B on main (release)");
    }
}
