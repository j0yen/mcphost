//! AC5 (PRD-mcphost-claude-code-plugin-and-snippets) — Given
//! `plugin/skills/mcphost/SKILL.md`, When grepped, Then it contains
//! `handoff`, `redeem`, `source: "plugin"`, `claim_url`, and the phrase
//! "never print the key".

const SKILL_MD: &str = include_str!("../plugin/skills/mcphost/SKILL.md");

#[test]
fn skill_md_has_frontmatter_name_and_description() {
    assert!(SKILL_MD.starts_with("---\n"), "SKILL.md must open with YAML frontmatter");
    let end = SKILL_MD[4..]
        .find("---")
        .map(|i| i + 4)
        .expect("SKILL.md frontmatter must be closed with a second '---'");
    let frontmatter = &SKILL_MD[..end];
    assert!(frontmatter.contains("name:"), "frontmatter must set name");
    assert!(frontmatter.contains("description:"), "frontmatter must set description");
}

#[test]
fn skill_md_walks_handoff_signup_and_redeem() {
    assert!(SKILL_MD.contains("handoff"), "SKILL.md must mention handoff mode");
    assert!(SKILL_MD.contains("redeem"), "SKILL.md must mention redeem");
    assert!(
        SKILL_MD.contains(r#"source: "plugin""#),
        "SKILL.md must tag signup with source: \"plugin\""
    );
}

#[test]
fn skill_md_relays_claim_url_and_never_prints_the_key() {
    assert!(SKILL_MD.contains("claim_url"), "SKILL.md must mention claim_url");
    assert!(
        SKILL_MD.contains("never print the key"),
        "SKILL.md must contain the phrase \"never print the key\""
    );
}
