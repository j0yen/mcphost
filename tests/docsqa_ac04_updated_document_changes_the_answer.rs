//! PRD-mcphost-docs-qa-recipe
//! AC4 -- Given a corpus document is updated to change the answer to
//! question 3, When the script re-asks after `lag_seconds == 0`, Then the
//! new passage is returned.
//!
//! Runs `docs-qa.sh` twice against the same tenant (`--reuse-state`) over a
//! scratch copy of the corpus (`--corpus-dir`, so the committed
//! `examples/docs-qa/corpus/` is never touched): once against the
//! unmodified `vpn-setup.md` (question 3's own document), then again after
//! rewriting its port number -- `host.docs.put` on the same `name` bumps
//! the document's version, and the second run's own poll for
//! `lag_seconds == 0` waits for that new version to actually reindex
//! before asking again.

use crate::common;
use crate::docs_qa;

use common::{TempDataDir, TestServer, python_kind_registry};

const OLD_FACT: &str = "51820";
const NEW_FACT: &str = "51821";

fn answer_for_question_3(receipt: &serde_json::Value) -> serde_json::Value {
    receipt["answers"]
        .as_array()
        .expect("answers array")
        .iter()
        .find(|a| a["id"] == 3)
        .cloned()
        .expect("question 3 must be present in the receipt")
}

#[tokio::test]
async fn reasking_after_a_document_update_returns_the_new_passage() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let endpoint = format!("{}/mcp", server.base_url);

    // A scratch copy of the corpus this test is free to mutate.
    let scratch = docs_qa::scratch_receipt_dir("ac04-corpus");
    for entry in std::fs::read_dir(docs_qa::corpus_dir()).expect("read corpus dir") {
        let entry = entry.expect("dir entry");
        let dest = scratch.join(entry.file_name());
        std::fs::copy(entry.path(), &dest).expect("copy corpus file");
    }
    let vpn_setup_path = scratch.join("vpn-setup.md");
    let original = std::fs::read_to_string(&vpn_setup_path).expect("read vpn-setup.md");
    assert!(
        original.contains(OLD_FACT),
        "fixture assumption: examples/docs-qa/corpus/vpn-setup.md must mention '{OLD_FACT}'"
    );

    let state_file = scratch.join("reuse-state.json");
    let receipt_dir_1 = docs_qa::scratch_receipt_dir("ac04-receipt-1");
    let receipt_dir_2 = docs_qa::scratch_receipt_dir("ac04-receipt-2");

    let run1 = docs_qa::run_docs_qa(
        &endpoint,
        &[
            "--corpus-dir",
            scratch.to_str().expect("utf8 path"),
            "--reuse-state",
            state_file.to_str().expect("utf8 path"),
            "--receipt-dir",
            receipt_dir_1.to_str().expect("utf8 path"),
        ],
        &[],
    )
    .await;
    assert!(run1.success, "first docs-qa.sh run must exit 0\nstdout:\n{}\nstderr:\n{}", run1.stdout, run1.stderr);
    let receipt1 = run1.receipt();
    let answer1 = answer_for_question_3(&receipt1);
    let text1 = answer1["passage_text"].as_str().expect("passage_text");
    assert!(text1.contains(OLD_FACT), "before the update, question 3's passage must contain '{OLD_FACT}': {text1}");
    assert!(!text1.contains(NEW_FACT), "before the update, question 3's passage must not already contain '{NEW_FACT}': {text1}");

    // Update the document that answers question 3.
    let updated = original.replace(OLD_FACT, NEW_FACT);
    assert_ne!(updated, original, "replacement must actually change the file");
    std::fs::write(&vpn_setup_path, updated).expect("write updated vpn-setup.md");

    let run2 = docs_qa::run_docs_qa(
        &endpoint,
        &[
            "--corpus-dir",
            scratch.to_str().expect("utf8 path"),
            "--reuse-state",
            state_file.to_str().expect("utf8 path"),
            "--receipt-dir",
            receipt_dir_2.to_str().expect("utf8 path"),
        ],
        &[],
    )
    .await;
    assert!(run2.success, "second docs-qa.sh run must exit 0\nstdout:\n{}\nstderr:\n{}", run2.stdout, run2.stderr);
    let receipt2 = run2.receipt();
    assert_eq!(
        receipt2["tenant"], receipt1["tenant"],
        "the second run must reuse the same tenant via --reuse-state, not sign up fresh"
    );
    let answer2 = answer_for_question_3(&receipt2);
    let text2 = answer2["passage_text"].as_str().expect("passage_text");
    assert!(
        text2.contains(NEW_FACT),
        "after the update and a lag_seconds == 0 re-ask, question 3's passage must contain the new fact '{NEW_FACT}': {text2}"
    );
    assert!(
        !text2.contains(OLD_FACT),
        "after the update, question 3's passage must no longer contain the stale fact '{OLD_FACT}': {text2}"
    );

    std::fs::remove_dir_all(&scratch).ok();
    std::fs::remove_dir_all(&receipt_dir_1).ok();
    std::fs::remove_dir_all(&receipt_dir_2).ok();
}
