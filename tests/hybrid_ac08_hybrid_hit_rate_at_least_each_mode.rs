//! PRD-mcphost-docs-hybrid-search
//! AC8 -- Given the docs-qa seeded corpus and gold questions, When hit
//! rate is measured for lexical, embeddings, and hybrid, Then hybrid is
//! at least each of the others.
//!
//! Reuses the same fixture `docsearch_ac09_fixture_corpus_hit_rate.rs`
//! replays (`tests/fixtures/docsearch/corpus.json`/`gold_questions.json`,
//! 20 documents/30 questions) -- the PRD's own grounding section points
//! at `docsearch`'s indexer/search code, and this is the repo's existing
//! offline corpus+gold fixture for that same recipe's hit-rate proof; no
//! new fixture is introduced.

use crate::common;
use mcphost::{docs, docs_index};
use serde::Deserialize;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

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
        "mcphost-hybrid-ac08-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// One keyword signature per corpus document -- present in both that
/// document's own prose (embedded once per chunk at index time) and its
/// gold questions' wording (embedded once per query at search time), same
/// "keyword -> fixed vector" trick `docsqa_ac07_embeddings_reports_both_hit_rates.rs`
/// uses for its own (smaller) corpus, sized to this fixture's 20 documents.
const SIGNATURES: &[(&str, &[&str])] = &[
    ("refund-policy.md", &["refund"]),
    ("shipping-times.md", &["standard shipping", "express shipping"]),
    ("return-process.md", &["returning", "thirty days", "return label"]),
    ("account-security.md", &["two-factor", "one-time code"]),
    ("password-reset.md", &["password", "reset"]),
    ("api-rate-limits.md", &["rate limit", "requests per hour", "429"]),
    ("api-authentication.md", &["bearer token", "api request", "authorization header"]),
    ("billing-cycle.md", &["billing cycle", "billed", "calendar day"]),
    ("subscription-cancellation.md", &["cancel", "cancellation"]),
    ("data-export.md", &["export", "zip archive"]),
    ("data-retention.md", &["retained", "retention", "backups"]),
    ("privacy-policy.md", &["personal data", "third parties"]),
    ("support-hours.md", &["hours", "business day"]),
    ("outage-status.md", &["outage", "status page", "incident"]),
    ("warranty-coverage.md", &["warranty"]),
    ("loyalty-points.md", &["loyalty"]),
    ("gift-cards.md", &["gift card"]),
    ("international-shipping.md", &["customs duties", "international shipping"]),
    ("accessibility-statement.md", &["accessibility"]),
];

fn fake_embedding(text: &str) -> Vec<f64> {
    let lower = text.to_lowercase();
    let mut vec: Vec<f64> = SIGNATURES
        .iter()
        .map(|(_, keywords)| keywords.iter().filter(|kw| lower.contains(**kw)).count() as f64)
        .collect();
    if vec.iter().all(|&v| v == 0.0) {
        vec = vec![0.01; SIGNATURES.len()];
    }
    vec
}

fn fake_embed_response(req: &Request) -> ResponseTemplate {
    let body: serde_json::Value = serde_json::from_slice(&req.body).expect("valid JSON body");
    let inputs = body["input"].as_array().expect("input array");
    let data: Vec<serde_json::Value> = inputs
        .iter()
        .map(|v| json!({"embedding": fake_embedding(v.as_str().unwrap_or(""))}))
        .collect();
    ResponseTemplate::new(200).set_body_json(json!({"data": data}))
}

async fn hit_rate(state: &mcphost::state::AppState, tenant: &mcphost::db::Tenant, gold: &[GoldQuestion], mode: &str) -> f64 {
    let mut hits = 0usize;
    for q in gold {
        let result = docs::doc_search(state, tenant, &json!({"query": q.query, "k": 5, "mode": mode}), None)
            .await
            .unwrap_or_else(|e| panic!("search failed for {:?} in mode {mode}: {e:?}", q.query));
        let names: Vec<String> = result["results"]
            .as_array()
            .expect("results array")
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect();
        if names.contains(&q.expected_document) {
            hits += 1;
        }
    }
    hits as f64 / gold.len() as f64
}

#[tokio::test]
async fn hybrid_hit_rate_is_at_least_lexical_and_embeddings_on_the_fixture() {
    let corpus_raw = std::fs::read_to_string("tests/fixtures/docsearch/corpus.json")
        .expect("read tests/fixtures/docsearch/corpus.json");
    let corpus: Vec<CorpusDoc> = serde_json::from_str(&corpus_raw).expect("parse corpus.json");

    let gold_raw = std::fs::read_to_string("tests/fixtures/docsearch/gold_questions.json")
        .expect("read tests/fixtures/docsearch/gold_questions.json");
    let gold: Vec<GoldQuestion> = serde_json::from_str(&gold_raw).expect("parse gold_questions.json");

    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hybrid-ac08").await;

    let provider = MockServer::start().await;
    Mock::given(method("POST")).respond_with(fake_embed_response).mount(&provider).await;

    let (ct, nonce) = state.secrets.encrypt("test-embed-secret-value").unwrap();
    state.db.upsert_secret(tenant.id, "EMBED_KEY".to_string(), ct, nonce).await.expect("upsert secret");
    docs::doc_index_config(
        &state,
        &tenant,
        &json!({
            "provider": "openai-compatible",
            "endpoint": format!("{}/v1/embeddings", provider.uri()),
            "model": "m",
            "secret": "EMBED_KEY",
        }),
    )
    .await
    .expect("index_config ok");

    for doc in &corpus {
        docs::doc_put(&state, &tenant, &json!({"name": doc.name, "content": doc.content}))
            .await
            .unwrap_or_else(|e| panic!("put {} failed: {e:?}", doc.name));
    }
    docs_index::tick_once(&state).await.expect("tick ok");

    let lexical_rate = hit_rate(&state, &tenant, &gold, "lexical").await;
    let embeddings_rate = hit_rate(&state, &tenant, &gold, "embeddings").await;
    let hybrid_rate = hit_rate(&state, &tenant, &gold, "hybrid").await;

    assert!(
        hybrid_rate >= lexical_rate,
        "hybrid hit rate {hybrid_rate:.2} must be at least lexical's {lexical_rate:.2}"
    );
    assert!(
        hybrid_rate >= embeddings_rate,
        "hybrid hit rate {hybrid_rate:.2} must be at least embeddings' {embeddings_rate:.2}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
