//! PRD-mcphost-docs-hybrid-search
//! AC10 -- Given `filter.prefix` set, When hybrid search runs, Then every
//! hit's `name` starts with the prefix and both candidate lists were
//! filtered before fusion.
//!
//! Proven, not just asserted: the excluded document is the strongest
//! match in BOTH raw lists (BM25 rank 1 and cosine rank 1) for this
//! query. If filtering ran AFTER fusion instead of before, the surviving
//! document would still be the only hit returned (the excluded one gets
//! stripped either way) but its own `ranks` would read `{lexical: 2,
//! embeddings: 2}` -- it would have been ranked *behind* the excluded
//! document in both raw lists before that document got removed. Filtering
//! before fusion (the requirement) means the excluded document never
//! occupies a rank slot at all, so the surviving hit's ranks are both 1.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hybrid-ac10-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn fake_embed_response(req: &Request) -> ResponseTemplate {
    let body: serde_json::Value = serde_json::from_slice(&req.body).expect("valid JSON body");
    let inputs = body["input"].as_array().expect("input array");
    let data: Vec<serde_json::Value> = inputs
        .iter()
        .map(|v| {
            let text = v.as_str().unwrap_or("");
            let embedding: Vec<f64> = if text.contains("STRONGMARKER") {
                vec![1.0, 0.0]
            } else if text.contains("WEAKMARKER") {
                vec![0.3, 0.953_939_2]
            } else {
                // The query itself.
                vec![1.0, 0.0]
            };
            json!({"embedding": embedding})
        })
        .collect();
    ResponseTemplate::new(200).set_body_json(json!({"data": data}))
}

#[tokio::test]
async fn filter_prefix_excludes_the_stronger_match_from_both_lists_before_fusion() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hybrid-ac10").await;

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

    // Excluded by filter.prefix below, but the strongest match in both
    // raw candidate lists: highest cosine (STRONGMARKER) and highest BM25
    // ("deadline" repeated, a rare term this doc alone owns).
    docs::doc_put(
        &state,
        &tenant,
        &json!({
            "name": "other/strong.md",
            "content": "Quarterly report deadline enforcement: the report deadline deadline \
                         deadline is fixed each quarter. STRONGMARKER",
        }),
    )
    .await
    .expect("put strong ok");

    // Matches filter.prefix "kb/" and matches the query weakly (BM25 via
    // "report" alone; cosine 0.3 via WEAKMARKER).
    docs::doc_put(
        &state,
        &tenant,
        &json!({
            "name": "kb/weak.md",
            "content": "Reports are reviewed periodically for quality. WEAKMARKER",
        }),
    )
    .await
    .expect("put weak ok");

    docs_index::tick_once(&state).await.expect("tick ok");

    let result = docs::doc_search(
        &state,
        &tenant,
        &json!({"query": "deadline report", "k": 5, "filter": {"prefix": "kb/"}}),
        None,
    )
    .await
    .expect("search ok");

    assert_eq!(result["index"]["mode"], json!("hybrid"));
    let results = result["results"].as_array().expect("results array");
    assert_eq!(results.len(), 1, "only the kb/-prefixed document must survive the filter: {results:?}");
    let hit = &results[0];
    assert!(
        hit["name"].as_str().unwrap().starts_with("kb/"),
        "every hit's name must start with the filter prefix: {hit:?}"
    );

    assert_eq!(
        hit["ranks"]["lexical"],
        json!(1),
        "with the stronger document filtered out before fusion, the survivor must be lexical rank 1, not 2: {hit:?}"
    );
    assert_eq!(
        hit["ranks"]["embeddings"],
        json!(1),
        "with the stronger document filtered out before fusion, the survivor must be embeddings rank 1, not 2: {hit:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
