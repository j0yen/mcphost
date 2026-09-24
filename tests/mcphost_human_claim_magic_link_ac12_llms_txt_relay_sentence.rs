//! PRD-mcphost-human-claim-magic-link
//! AC12 (P1) — Given www/llms.txt on the built tree, When grepped, Then it
//! contains the relay sentence with the literal `claim_url` placeholder
//! and "7 days".

const LLMS_TXT: &str = include_str!("../www/llms.txt");

#[test]
fn llms_txt_contains_the_claim_relay_sentence() {
    assert!(
        LLMS_TXT.contains("claim_url"),
        "www/llms.txt must name the literal claim_url placeholder"
    );
    assert!(
        LLMS_TXT.contains("7 days"),
        "www/llms.txt must state the claim link's 7-day expiry"
    );
    assert!(
        LLMS_TXT.contains("Claim this backend so it belongs to you"),
        "www/llms.txt must carry the exact relay sentence the agent hands to its human"
    );
}
