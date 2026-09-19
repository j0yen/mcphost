//! PRD-mcphost-share-a-tool-not-a-key
//! AC9 — Given the public variant section, When read, Then it states why
//! group visibility is the default.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

const SECTION_HEADING: &str = "### Share a tool, not a key: public variant";

fn section() -> &'static str {
    let start = LLMS_TXT
        .find(SECTION_HEADING)
        .expect("www/llms.txt must have a 'public variant' section");
    let rest = &LLMS_TXT[start..];
    let end = rest[SECTION_HEADING.len()..]
        .find("\n## ")
        .map(|offset| SECTION_HEADING.len() + offset)
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn public_variant_section_exists_and_shows_the_public_call() {
    let section = section();
    assert!(
        section.contains("visibility=\"public\"") || section.contains("visibility: \"public\""),
        "public variant section must show the visibility=public call: {section}"
    );
}

#[test]
fn public_variant_section_states_why_group_is_the_default() {
    let section = section();
    // The reasoning must actually be present, not just a bare pointer to a
    // decision made elsewhere: name the concrete risk (shared/paid upstream
    // exposed to uncontrolled call volume) that makes `group` the default.
    assert!(
        section.contains("default"),
        "section must call out that `group` is the default: {section}"
    );
    let lower = section.to_ascii_lowercase();
    let names_the_risk = lower.contains("rate limit")
        || lower.contains("paid")
        || lower.contains("bill")
        || lower.contains("upstream account");
    assert!(
        names_the_risk,
        "section must state *why* group is the default (the shared-upstream/paid-key risk \
         `public` would expose), not just assert that it is: {section}"
    );
}
