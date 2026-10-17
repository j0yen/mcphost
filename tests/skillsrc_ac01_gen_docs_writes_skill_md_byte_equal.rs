//! PRD-mcphost-skill-md-generated-from-plugin
//! AC1 (P0) -- Given the plugin skill file, When `mcphost gen-docs` runs,
//! Then `www/skill.md` is written byte-equal to it with no added header,
//! trailer, or newline change.

use crate::common;

#[test]
fn skillsrc_ac01() {
    let dir = common::gen_docs_scratch_tree("skillsrc-ac01");
    std::fs::write(dir.join("www/skill.md"), "stale\n").unwrap();
    let out = common::run_gen_docs(&dir, &[]);
    assert!(out.status.success(), "gen-docs failed: {}", String::from_utf8_lossy(&out.stderr));
    let written = std::fs::read(dir.join("www/skill.md")).unwrap();
    let plugin = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/plugin/skills/mcphost/SKILL.md")).unwrap();
    assert_eq!(written, plugin, "www/skill.md must be byte-equal to the plugin skill");
}
