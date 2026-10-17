//! PRD-mcphost-skill-md-generated-from-plugin
//! AC2 (P0) -- Given `www/skill.md` differing from the plugin file by one
//! byte, When `mcphost gen-docs --check` runs, Then it exits 1 and names
//! `www/skill.md`; Given the two equal, Then it exits 0.

use crate::common;

#[test]
fn skillsrc_ac02() {
    let dir = common::gen_docs_scratch_tree("skillsrc-ac02");
    let out = common::run_gen_docs(&dir, &["--check"]);
    assert_eq!(out.status.code(), Some(0), "equal tree: {}", String::from_utf8_lossy(&out.stderr));

    let mut bytes = std::fs::read(dir.join("www/skill.md")).unwrap();
    bytes.push(b'x');
    std::fs::write(dir.join("www/skill.md"), bytes).unwrap();
    let out = common::run_gen_docs(&dir, &["--check"]);
    assert_eq!(out.status.code(), Some(1));
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("www/skill.md"), "output must name www/skill.md: {text}");
}
