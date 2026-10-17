//! PRD-mcphost-skill-md-generated-from-plugin
//! AC5 (P0) -- Given the `gen-docs` output list, When read through its
//! public accessor, Then it contains `www/skill.md` beside `docs/tools.md`,
//! `docs/plans.md`, and `docs/plans.html`, and the docurl_ac02 byte-identity
//! test is still present.

#[test]
fn skillsrc_ac05() {
    let paths = mcphost::gendocs::output_paths();
    for want in ["www/skill.md", "docs/tools.md", "docs/plans.md", "www/plans.html"] {
        assert!(paths.contains(&want), "gen-docs output list lacks {want}: {paths:?}");
    }
    let drift = include_str!("docurl_ac02_skill_one_url_no_ritual.rs");
    assert!(drift.contains("fn www_skill_md_is_byte_identical_to_the_plugin_skill"));
}
