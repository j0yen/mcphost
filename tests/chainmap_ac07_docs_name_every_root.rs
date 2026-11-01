//! PRD-mcphost-chain-near-miss-mapping-refused AC7 (P1) -- Given
//! `docs/kinds/chain.md`, When the docs test reads its near-miss sentence,
//! Then the sentence says a value starting with a context root without `$.` is
//! refused at publish as `not a path` and names every root in `CONTEXT_KEYS`,
//! failing if a root is added to the table and not to the sentence.

use mcphost::kinds::chain::{CONTEXT_KEYS, near_miss_sentence};

#[test]
fn the_doc_states_the_near_miss_sentence_and_names_every_root() {
    let sentence = near_miss_sentence();
    for root in [
        CONTEXT_KEYS.input,
        CONTEXT_KEYS.prev,
        CONTEXT_KEYS.steps,
        CONTEXT_KEYS.event,
        CONTEXT_KEYS.result,
        CONTEXT_KEYS.item,
        CONTEXT_KEYS.index,
    ] {
        assert!(sentence.contains(&format!("`{root}`")), "the sentence must name `{root}`: {sentence}");
    }
    for part in ["without `$.`", "refused at publish", "`not a path`"] {
        assert!(sentence.contains(part), "the sentence must say {part}: {sentence}");
    }

    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/kinds/chain.md"))
        .expect("docs/kinds/chain.md");
    assert!(doc.contains(&sentence), "docs/kinds/chain.md must state: {sentence}");
}
