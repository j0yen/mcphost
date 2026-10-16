//! PRD-less hand build: scripts/inner-rules.sh -- the generated coding-rules
//! block wm-build injects into every inner-coder prompt.
//! AC1 -- Given the script, When it runs, Then its output is non-empty,
//! mentions `flake-lint` and every test named in its `CONTRACT_TESTS` array,
//! and every `tests/<name>.rs` in that array exists on disk.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Parse the `CONTRACT_TESTS=( ... )` array out of the script source.
fn contract_tests() -> Vec<String> {
    let src = fs::read_to_string(crate_root().join("scripts/inner-rules.sh")).expect("read inner-rules.sh");
    let mut in_array = false;
    let mut names = Vec::new();
    for line in src.lines() {
        let t = line.trim();
        if !in_array {
            in_array = t.starts_with("CONTRACT_TESTS=(");
            continue;
        }
        if t == ")" {
            break;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        names.push(t.to_string());
    }
    names
}

#[test]
fn inner_rules_block_names_every_contract_test() {
    let tests = contract_tests();
    assert!(!tests.is_empty(), "CONTRACT_TESTS array must not be empty");

    let out = Command::new("bash")
        .arg(crate_root().join("scripts/inner-rules.sh"))
        .output()
        .expect("run inner-rules.sh");
    assert!(out.status.success(), "inner-rules.sh failed: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout).to_string();

    assert!(!text.trim().is_empty(), "inner-rules.sh printed nothing");
    assert!(text.contains("flake-lint"), "block must mention flake-lint");
    assert!(text.contains("Done checks the daemon runs:"), "block must end with the done-checks line");
    for name in &tests {
        let file = crate_root().join("tests").join(format!("{name}.rs"));
        assert!(file.is_file(), "CONTRACT_TESTS names tests/{name}.rs which does not exist");
        assert!(text.contains(name.as_str()), "block does not mention {name}");
        assert!(!text.contains(&format!("tests/{name}.rs] MISSING")), "block reports {name} MISSING");
    }
}
