//! PRD-mcphost-docs-qa-recipe
//! AC1 -- Given the in-process test host, When
//! `examples/docs-qa/docs-qa.sh <url>` runs, Then it exits 0, the receipt
//! shows `index_ready_secs < 30`, `hits >= 5`, and every answer carries a
//! `name:offset` citation.

use crate::common;
use crate::docs_qa;

use common::{TempDataDir, TestServer, python_kind_registry};

#[tokio::test]
async fn recipe_exits_zero_with_a_green_receipt() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let endpoint = format!("{}/mcp", server.base_url);
    let receipt_dir = docs_qa::scratch_receipt_dir("ac01");

    let run = docs_qa::run_docs_qa(
        &endpoint,
        &["--receipt-dir", receipt_dir.to_str().expect("utf8 path")],
        &[],
    )
    .await;

    assert!(run.success, "docs-qa.sh must exit 0\nstdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);

    let receipt = run.receipt();
    let index_ready_secs = receipt["index_ready_secs"].as_f64().expect("index_ready_secs");
    assert!(
        index_ready_secs < 30.0,
        "index_ready_secs must be under 30s, got {index_ready_secs}: {receipt}"
    );

    let hits = receipt["hits"].as_i64().expect("hits");
    assert!(hits >= 5, "expected at least 5/6 hits, got {hits}: {receipt}");

    let gold = docs_qa::load_gold();
    let md_count = std::fs::read_dir(docs_qa::corpus_dir())
        .expect("read corpus dir")
        .filter(|e| e.as_ref().is_ok_and(|e| e.path().extension().is_some_and(|ext| ext == "md")))
        .count();
    assert_eq!(md_count, 8, "requirement 1: the corpus is 8 markdown documents");
    assert_eq!(gold.len(), 6, "requirement 1: gold.json has 6 questions");

    let answers = receipt["answers"].as_array().expect("answers array");
    assert_eq!(answers.len(), 6, "expected 6 answers, one per gold question: {receipt}");
    for (answer, gold_q) in answers.iter().zip(gold.iter()) {
        assert_eq!(answer["id"].as_i64(), Some(gold_q.id), "answers must be in gold.json order: {receipt}");
        assert_eq!(
            answer["expected_document"].as_str(),
            Some(gold_q.expected_document.as_str()),
            "answer must echo gold.json's own expected_document: {receipt}"
        );
    }
    for answer in answers {
        let citation = answer["citation"].as_str().unwrap_or_else(|| {
            panic!("every answer must carry a citation, question {}: {receipt}", answer["id"])
        });
        let (name, offset) = citation
            .split_once(':')
            .unwrap_or_else(|| panic!("citation '{citation}' must be 'name:offset'"));
        assert!(name.ends_with(".md"), "citation name '{name}' must name a corpus document");
        offset
            .parse::<i64>()
            .unwrap_or_else(|e| panic!("citation offset '{offset}' must be an integer: {e}"));
    }

    std::fs::remove_dir_all(&receipt_dir).ok();
}
