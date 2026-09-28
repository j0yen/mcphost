//! PRD-mcphost-docs-qa-recipe
//! AC7 -- Given `--embeddings` with a test provider, When the script runs,
//! Then the receipt reports hit rates for both lexical and embeddings
//! modes.

use crate::common;
use crate::docs_qa;

use common::{TempDataDir, TestServer, python_kind_registry};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// One keyword signature per corpus document -- present in both that
/// document's own prose (embedded once per chunk at index time) and its
/// gold question's wording (embedded once per query at search time), so a
/// deterministic fake embedder can rank the right document first without a
/// real model, same "keyword -> fixed vector" trick
/// `docsearch_ac05_embeddings_mode_ranks_by_cosine.rs` uses for its own two
/// documents, generalized to this corpus's 8.
const SIGNATURES: &[(&str, &[&str])] = &[
    ("onboarding.md", &["onboarding", "paperwork", "new hire"]),
    ("expense-policy.md", &["reimbursement", "receipt", "meal"]),
    ("vpn-setup.md", &["vpn", "port", "wireguard"]),
    ("incident-response.md", &["sev1", "incident", "paged"]),
    ("pto-policy.md", &["pto", "accrue", "vacation"]),
    ("security-basics.md", &["password", "rotate"]),
    ("api-rate-limits.md", &["rate limit", "429"]),
    ("shipping-faq.md", &["shipping", "tracking"]),
];

fn fake_embedding(text: &str) -> Vec<f64> {
    let lower = text.to_lowercase();
    let mut vec: Vec<f64> = SIGNATURES
        .iter()
        .map(|(_, keywords)| keywords.iter().filter(|kw| lower.contains(**kw)).count() as f64)
        .collect();
    if vec.iter().all(|&v| v == 0.0) {
        // No signature keyword present -- a small uniform vector so cosine
        // is still well-defined (never the zero vector) without favoring
        // any one document.
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

#[tokio::test]
async fn embeddings_flag_reports_both_lexical_and_embeddings_hit_rates() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let endpoint = format!("{}/mcp", server.base_url);
    let receipt_dir = docs_qa::scratch_receipt_dir("ac07");

    let provider = MockServer::start().await;
    Mock::given(method("POST")).respond_with(fake_embed_response).mount(&provider).await;
    let provider_endpoint = format!("{}/v1/embeddings", provider.uri());

    let run = docs_qa::run_docs_qa(
        &endpoint,
        &[
            "--receipt-dir",
            receipt_dir.to_str().expect("utf8 path"),
            "--embeddings",
            &provider_endpoint,
            "test-embed-model",
            "EMBED_KEY",
        ],
        &[("DOCS_QA_EMBED_SECRET_VALUE", "test-embed-secret-value")],
    )
    .await;
    assert!(run.success, "docs-qa.sh --embeddings must exit 0\nstdout:\n{}\nstderr:\n{}", run.stdout, run.stderr);

    let receipt = run.receipt();
    let modes = &receipt["modes"];
    assert!(modes.is_object(), "receipt must carry a modes section: {receipt}");

    let lexical = &modes["lexical"];
    let lexical_hits = lexical["hits"].as_i64().expect("modes.lexical.hits");
    let lexical_total = lexical["total"].as_i64().expect("modes.lexical.total");
    assert_eq!(lexical_total, 6);
    assert!(lexical_hits >= 5, "lexical hit rate too low: {lexical_hits}/{lexical_total}: {receipt}");

    let embeddings = &modes["embeddings"];
    let embeddings_hits = embeddings["hits"].as_i64().expect("modes.embeddings.hits");
    let embeddings_total = embeddings["total"].as_i64().expect("modes.embeddings.total");
    assert_eq!(embeddings_total, 6);
    assert!(
        embeddings_hits >= 5,
        "embeddings hit rate too low: {embeddings_hits}/{embeddings_total}: {receipt}"
    );

    // The provider actually got dispatched to (not a silent lexical
    // fallback) -- at least one indexing request plus at least one query
    // request per gold question.
    let received = provider.received_requests().await.expect("requests recorded");
    assert!(!received.is_empty(), "the embeddings provider must have received requests");

    std::fs::remove_dir_all(&receipt_dir).ok();
}
