//! PRD-mcphost-docs-examples-executed-nightly AC1 (P0) -- Given the fenced
//! blocks of `www/llms-full.txt`, When `docs-examples classify` runs, Then
//! it prints one line per fence with `runnable|fragment|prose` and >= 40
//! are `runnable`; a lower count fails the test and prints the fragment
//! list.
//!
//! Note on "109 fences": the PRD counted fence DELIMITER lines (108 at
//! 4e07dfd, an opening and a closing per block); the file holds 54 blocks,
//! which is what `classify` counts.

use std::process::Command;

#[test]
fn classify_prints_one_line_per_fence_and_at_least_40_are_runnable() {
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["docs-examples", "classify", "--path", "www/llms-full.txt"])
        .output()
        .expect("run mcphost docs-examples classify");
    assert!(out.status.success(), "classify failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();

    let text = std::fs::read_to_string(format!("{}/www/llms-full.txt", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let delimiters = text.lines().filter(|l| l.trim_start().starts_with("```")).count();
    assert_eq!(lines.len() * 2, delimiters, "one output line per fence (each fence = 2 delimiter lines)");

    for l in &lines {
        let class = l.rsplit(' ').next().unwrap();
        assert!(["runnable", "fragment", "prose"].contains(&class), "bad classification line: {l}");
    }
    let runnable = lines.iter().filter(|l| l.ends_with(" runnable")).count();
    let not_runnable: Vec<&&str> = lines.iter().filter(|l| !l.ends_with(" runnable")).collect();
    assert!(
        runnable >= 40,
        "only {runnable} of {} fences are runnable (need >= 40); fragment list:\n{}",
        lines.len(),
        not_runnable.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")
    );
}
