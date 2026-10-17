//! AC2 (PRD-mcphost-plugin-skill-generated-from-tools-list) -- Given the
//! checked-in files, When `mcphost skill-gen` runs without `--write`, Then it
//! exits 0; Given one verb description edited by hand inside a marked
//! section, Then it exits 1 and names the line.

use std::path::PathBuf;
use std::process::{Command, Output};

fn skill_gen(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost")).arg("skill-gen").args(args).output().expect("run mcphost skill-gen")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("skgn-ac02-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn skgn_ac02() {
    // Checked-in files: exit 0 (cargo runs integration tests from the repo root).
    let clean = skill_gen(&[]);
    assert_eq!(
        clean.status.code(),
        Some(0),
        "skill-gen on the checked-in files must exit 0: {}",
        String::from_utf8_lossy(&clean.stderr)
    );

    // One verb description edited by hand inside the marked section.
    let dir = scratch("edit");
    let plugin = dir.join("SKILL.md");
    let www = dir.join("skill.md");
    let original = std::fs::read_to_string(mcphost::skill_gen::SKILL_PATH).expect("read plugin skill");
    std::fs::copy(mcphost::skill_gen::WWW_SKILL_PATH, &www).expect("copy www skill");

    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let start = lines.iter().position(|l| l.contains("mcphost-verbs:start")).expect("verbs start marker");
    let edited = (start..lines.len())
        .find(|i| lines[*i].starts_with("- `host.whoami` -- "))
        .expect("host.whoami verb line");
    lines[edited] = format!("{} (hand edit)", lines[edited]);
    std::fs::write(&plugin, lines.join("\n") + "\n").expect("write edited skill");

    let out = skill_gen(&["--plugin", plugin.to_str().unwrap(), "--www", www.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "a hand-edited verb line must exit 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let named = format!("{}:{}", plugin.display(), edited + 1);
    assert!(stderr.contains(&named), "stderr must name {named}: {stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("(hand edit)"), "stdout is the rendered (regenerated) file");

    // `--write` repairs it, after which the check passes again.
    let write = skill_gen(&["--write", "--plugin", plugin.to_str().unwrap(), "--www", www.to_str().unwrap()]);
    assert_eq!(write.status.code(), Some(0));
    let again = skill_gen(&["--plugin", plugin.to_str().unwrap(), "--www", www.to_str().unwrap()]);
    assert_eq!(again.status.code(), Some(0), "{}", String::from_utf8_lossy(&again.stderr));
    let _ = std::fs::remove_dir_all(&dir);
}
