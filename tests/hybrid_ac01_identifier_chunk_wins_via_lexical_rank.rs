//! PRD-mcphost-docs-hybrid-search
//! AC1 -- Given a tenant with a configured provider and a corpus where an
//! identifier-bearing chunk ranks 12th by cosine and 1st by BM25, When
//! `host.docs.search` runs with `k: 5` and default mode, Then that chunk
//! is in the top 5, `index.mode` is `hybrid`, and its `ranks` shows both
//! positions.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hybrid-ac01-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A deterministic fake embedder that deliberately disagrees with BM25:
/// the target document's own chunk text (marked `TARGETMARKER`) embeds far
/// from the query, while every distractor (marked `DISTRACTOR`) embeds
/// identical to the query -- 11 distractors guarantee the target's cosine
/// rank is exactly 12th among the 12 vectors, regardless of tie order
/// among the (all cosine-1.0) distractors.
fn fake_embed_response(req: &Request) -> ResponseTemplate {
    let body: serde_json::Value = serde_json::from_slice(&req.body).expect("valid JSON body");
    let inputs = body["input"].as_array().expect("input array");
    let data: Vec<serde_json::Value> = inputs
        .iter()
        .map(|v| {
            let text = v.as_str().unwrap_or("");
            let embedding: Vec<f64> = if text.contains("TARGETMARKER") {
                vec![-1.0, 0.0]
            } else {
                // Every distractor chunk, and the query itself, land here.
                vec![1.0, 0.0]
            };
            json!({"embedding": embedding})
        })
        .collect();
    ResponseTemplate::new(200).set_body_json(json!({"data": data}))
}

#[tokio::test]
async fn identifier_chunk_ranked_12th_by_cosine_still_lands_in_top5_via_bm25() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hybrid-ac01").await;

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

    // The only document whose text contains the rare identifier "20419" --
    // BM25 ranks it 1st for the query below despite the fake embedder
    // deliberately ranking it last (12th) by cosine.
    docs::doc_put(
        &state,
        &tenant,
        &json!({
            "name": "invoice-inv-20419.md",
            "content": "Invoice INV-20419 due date reminder TARGETMARKER: payment for \
                         invoice INV-20419 is due at the end of the month.",
        }),
    )
    .await
    .expect("put target ok");

    for i in 0..11 {
        docs::doc_put(
            &state,
            &tenant,
            &json!({
                "name": format!("distractor-{i:02}.md"),
                "content": format!(
                    "DISTRACTOR filler passage number {i} about unrelated topics, padding \
                     out the corpus with prose that shares no query terms."
                ),
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("put distractor {i} failed: {e:?}"));
    }

    docs_index::tick_once(&state).await.expect("tick ok");

    let result = docs::doc_search(
        &state,
        &tenant,
        &json!({"query": "invoice INV-20419 due date", "k": 5}),
        None,
    )
    .await
    .expect("search ok");

    assert_eq!(result["index"]["mode"], json!("hybrid"), "default mode with a configured provider must fuse: {result:?}");

    let results = result["results"].as_array().expect("results array");
    let hit = results
        .iter()
        .find(|r| r["name"] == json!("invoice-inv-20419.md"))
        .unwrap_or_else(|| panic!("identifier chunk missing from top 5: {results:?}"));

    assert_eq!(hit["ranks"]["lexical"], json!(1), "BM25 must rank the identifier chunk 1st: {hit:?}");
    assert_eq!(hit["ranks"]["embeddings"], json!(12), "cosine must rank the identifier chunk 12th: {hit:?}");

    std::fs::remove_dir_all(&dir).ok();
}
