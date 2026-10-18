//! contract: reads scripts/contract-lint.sh
//! PRD-mcphost-contract-suite-registry-axis-b AC7 -- `scripts/inner-rules.sh`
//! prints, in its contract section, one `<path | registry sym> \u{2014} <definer
//! stem>: <marker text>` line per `scripts/contract-lint.sh --definers` entry;
//! the script source has no hand array; the tail names `scripts/pin-tests.sh`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run(script: &str, args: &[&str]) -> String {
    let out = Command::new("bash").arg(root().join(script)).args(args).current_dir(root()).output().unwrap();
    assert!(out.status.success(), "{script}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn ctrx_ac07_one_line_per_definers_entry_with_definer_stem_and_marker_text() {
    let block = run("scripts/inner-rules.sh", &[]);
    let section = block.split("-- 2. Contract rules").nth(1).expect("contract section");
    let section = section.split("Done checks the daemon runs:").next().unwrap();
    let definers = run("scripts/contract-lint.sh", &["--definers", "tests"]);
    assert!(definers.lines().any(|l| l.starts_with("registry ")), "{definers}");
    for l in definers.lines() {
        let mut cols = l.split('\t');
        let (entry, file, text) = (cols.next().unwrap(), cols.next().unwrap(), cols.next().unwrap());
        let stem = std::path::Path::new(file).file_stem().unwrap().to_string_lossy().into_owned();
        assert!(root().join(format!("tests/{stem}.rs")).is_file(), "definer {stem} is not a test file: {l}");
        assert!(!text.trim().is_empty(), "empty marker text: {l}");
        let want = format!("{entry} \u{2014} {stem}: {text}");
        assert!(section.lines().any(|b| b == want), "no line `{want}` in:\n{section}");
    }
    let entry_lines = section.lines().filter(|l| l.contains(" \u{2014} ")).count();
    assert_eq!(entry_lines, definers.lines().count(), "{section}");
}

#[test]
fn ctrx_ac07_no_hand_array_and_tail_names_pin_tests() {
    let src = fs::read_to_string(root().join("scripts/inner-rules.sh")).unwrap();
    assert!(!src.contains(&format!("{}=(", "CONTRACT_TESTS")));
    let block = run("scripts/inner-rules.sh", &[]);
    let tail = block.split("Done checks the daemon runs:").nth(1).expect("tail");
    assert!(tail.contains("scripts/pin-tests.sh") && tail.contains("scripts/flake-lint.sh"), "{tail}");
}
