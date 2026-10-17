//! PRD-mcphost-skill-md-generated-from-plugin
//! AC3 (P0) -- Given `www/skill.md` absent, When `mcphost gen-docs --check`
//! runs, Then it exits 1 naming `www/skill.md`, and a following `gen-docs`
//! recreates it byte-equal.

use crate::common;

#[test]
fn skillsrc_ac03() {
    let dir = common::gen_docs_scratch_tree("skillsrc-ac03");
    std::fs::remove_file(dir.join("www/skill.md")).unwrap();
    let out = common::run_gen_docs(&dir, &["--check"]);
    assert_eq!(out.status.code(), Some(1));
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("www/skill.md"), "output must name www/skill.md: {text}");

    let out = common::run_gen_docs(&dir, &[]);
    assert!(out.status.success(), "gen-docs failed: {}", String::from_utf8_lossy(&out.stderr));
    let written = std::fs::read(dir.join("www/skill.md")).unwrap();
    let plugin = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/plugin/skills/mcphost/SKILL.md")).unwrap();
    assert_eq!(written, plugin);
}
