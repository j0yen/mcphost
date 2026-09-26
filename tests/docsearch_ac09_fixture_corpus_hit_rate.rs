//! PRD-mcphost-docs-semantic-search
//! AC9 -- Given the fixture corpus and 30 gold questions, When the
//! hit-rate test runs in lexical mode, Then top-5 hit rate ≥ 0.9.
//!
//! Fixture corpus: `tests/fixtures/docsearch/corpus.json` (20 documents)
//! and `tests/fixtures/docsearch/gold_questions.json` (30 questions, each
//! naming the document that answers it) -- requirement 6's own repo
//! fixture, routed through `agent/proof-lanes.toml`'s `docsearch-fixtures`
//! lane the same way PRD-mcphost-wasm-kind's own `tests/fixtures/wasm*/**`
//! is routed through `wasm-fixtures`.

use crate::common;
use mcphost::{docs, docs_index};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct CorpusDoc {
    name: String,
    content: String,
}

#[derive(Deserialize)]
struct GoldQuestion {
    query: String,
    expected_document: String,
}

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac09-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn lexical_top5_hit_rate_is_at_least_90_percent() {
    let corpus_raw = std::fs::read_to_string("tests/fixtures/docsearch/corpus.json")
        .expect("read tests/fixtures/docsearch/corpus.json");
    let corpus: Vec<CorpusDoc> = serde_json::from_str(&corpus_raw).expect("parse corpus.json");
    assert_eq!(corpus.len(), 20, "requirement 6: the fixture corpus is 20 documents");

    let gold_raw = std::fs::read_to_string("tests/fixtures/docsearch/gold_questions.json")
        .expect("read tests/fixtures/docsearch/gold_questions.json");
    let gold: Vec<GoldQuestion> = serde_json::from_str(&gold_raw).expect("parse gold_questions.json");
    assert_eq!(gold.len(), 30, "requirement 6: 30 gold questions");

    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docsearch-ac09").await;

    for doc in &corpus {
        docs::doc_put(&state, &tenant, &json!({"name": doc.name, "content": doc.content}))
            .await
            .unwrap_or_else(|e| panic!("put {} failed: {e:?}", doc.name));
    }
    docs_index::tick_once(&state).await.expect("tick ok");

    let mut hits = 0usize;
    let mut misses = Vec::new();
    for q in &gold {
        let result = docs::doc_search(&state, &tenant, &json!({"query": q.query, "k": 5}))
            .await
            .expect("search ok");
        let names: Vec<String> = result["results"]
            .as_array()
            .expect("results array")
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect();
        if names.contains(&q.expected_document) {
            hits += 1;
        } else {
            misses.push((q.query.clone(), q.expected_document.clone(), names));
        }
    }

    let hit_rate = hits as f64 / gold.len() as f64;
    assert!(
        hit_rate >= 0.9,
        "top-5 hit rate {hit_rate:.2} ({hits}/{}) is below 0.9; misses: {misses:?}",
        gold.len()
    );

    std::fs::remove_dir_all(&dir).ok();
}
