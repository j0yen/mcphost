//! contract: reads plugin/skills/mcphost/SKILL.md
//! PRD-mcphost-plugin-skill-generated-from-registry
//! AC2 (P0) -- Given the checked-in files, When `mcphost skill-gen` runs
//! without `--write`, Then it exits 0; Given one line edited by hand inside a
//! marked section, Then it exits 1 and names the line.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn skill_gen(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("skill-gen")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run mcphost skill-gen")
}

#[test]
fn skgn_ac02_check_names_hand_edited_line() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = skill_gen(&root, &[]);
    assert_eq!(out.status.code(), Some(0), "checked-in files: {}", String::from_utf8_lossy(&out.stderr));

    let text = std::fs::read_to_string(root.join("plugin/skills/mcphost/SKILL.md")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| *l == "<!-- mcphost-verbs:start -->").expect("start marker");
    let end = lines.iter().position(|l| *l == "<!-- mcphost-verbs:end -->").expect("end marker");
    let victim = (start + 1..end).find(|&i| lines[i].starts_with("- `")).expect("a family line");
    let edited: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| if i == victim { format!("{l} HAND-EDIT") } else { l.to_string() })
        .collect();

    let dir = std::env::temp_dir().join(format!("skgn-ac02-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("SKILL.md");
    std::fs::write(&file, edited.join("\n") + "\n").unwrap();

    let out = skill_gen(&dir, &["--file", file.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "hand edit must exit 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&format!("line {}", victim + 1)), "must name line {}: {stderr}", victim + 1);
    assert!(stderr.contains("HAND-EDIT"), "must quote the edited line: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
