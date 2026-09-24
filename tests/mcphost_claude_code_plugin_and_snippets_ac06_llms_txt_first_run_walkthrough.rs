//! AC6 (PRD-mcphost-claude-code-plugin-and-snippets) — Given
//! `www/llms.txt`, When grepped, Then it contains the first-run
//! walk-through headings (signup, publish, schedule, claim) in that
//! order.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

#[test]
fn first_run_headings_appear_in_order() {
    let headings = ["signup", "publish", "schedule", "claim"];
    let lower = LLMS_TXT.to_ascii_lowercase();
    let mut last_idx: Option<usize> = None;
    for heading in headings {
        let idx = lower
            .find(&format!("### {heading}"))
            .unwrap_or_else(|| panic!("www/llms.txt must have a '### {heading}' heading"));
        if let Some(prev) = last_idx {
            assert!(
                idx > prev,
                "heading '### {heading}' must appear after the previous walk-through heading"
            );
        }
        last_idx = Some(idx);
    }
}

#[test]
fn claim_relay_sentence_is_present() {
    assert!(
        LLMS_TXT.contains("Claim this backend so it belongs to you"),
        "www/llms.txt must carry the claim relay sentence"
    );
    assert!(LLMS_TXT.contains("claim_url"));
}
