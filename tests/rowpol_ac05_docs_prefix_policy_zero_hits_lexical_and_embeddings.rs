//! PRD-mcphost-row-policy
//! AC5 (P0) — Given a docs policy on prefix `hr/` for role `hr`, When a
//! user without the role searches a term that only `hr/` documents
//! contain, Then zero hits return in `lexical`, `embeddings`, and
//! `hybrid` modes.

use crate::common;
use mcphost::enduser::{EndUser, EndUserMethod};
use mcphost::{docs, docs_index, rowpolicy};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-rowpol-ac05-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn dave() -> EndUser {
    EndUser {
        subject: "dave".to_string(),
        issuer: None,
        method: EndUserMethod::Assertion,
        verified_at: 0,
        email: None,
        name: None,
    }
}

#[tokio::test]
async fn no_role_gets_zero_hits_in_lexical_mode() {
    let dir = scratch_dir("lexical");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "rowpol-ac05-lexical").await;

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "hr/comp.md", "content": "the salarybandalpha applies to managers only"}),
    )
    .await
    .expect("put hr doc ok");
    docs_index::tick_once(&state).await.expect("tick ok");

    rowpolicy::policy_set(
        &state,
        &tenant,
        &json!({
            "target": {"doc_prefix": "hr/"},
            "rule": [{"column_or_attr": "role", "op": "eq", "value": {"literal": "hr"}}],
        }),
        None,
    )
    .await
    .expect("policy set ok");
    // Dave deliberately never gets a "role" attribute at all.

    let end_user = dave();
    let result = docs::doc_search(&state, &tenant, &json!({"query": "salarybandalpha", "k": 5}), Some(&end_user))
        .await
        .expect("search ok");
    assert_eq!(result["index"]["mode"], json!("lexical"), "{result:?}");
    assert!(
        result["results"].as_array().expect("results array").is_empty(),
        "a user without the hr role must get zero hits on an hr-only term: {result:?}"
    );

    // Sanity check against an always-deny bug: a subject *with* the hr
    // role must still see the hit.
    rowpolicy::policy_attrs_set(
        &state,
        &tenant,
        &json!({"subject": "hannah", "attrs": {"role": "hr"}}),
        None,
    )
    .await
    .expect("attrs set ok");
    let hannah = EndUser { subject: "hannah".to_string(), ..dave() };
    let hannah_result =
        docs::doc_search(&state, &tenant, &json!({"query": "salarybandalpha", "k": 5}), Some(&hannah))
            .await
            .expect("search ok");
    assert_eq!(
        hannah_result["results"].as_array().expect("results array").len(),
        1,
        "a subject with the hr role must still see the hr-only hit: {hannah_result:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn no_role_gets_zero_hits_in_embeddings_mode() {
    let dir = scratch_dir("embeddings");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "rowpol-ac05-embeddings").await;

    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).expect("valid JSON body");
            let inputs = body["input"].as_array().expect("input array");
            let data: Vec<serde_json::Value> =
                inputs.iter().map(|_| json!({"embedding": [1.0, 0.0]})).collect();
            ResponseTemplate::new(200).set_body_json(json!({"data": data}))
        })
        .mount(&provider)
        .await;
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

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "hr/comp.md", "content": "the salarybandalpha applies to managers only"}),
    )
    .await
    .expect("put hr doc ok");
    docs_index::tick_once(&state).await.expect("tick ok");

    let status = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    assert_eq!(status["index"]["mode"], json!("embeddings"), "{status:?}");

    rowpolicy::policy_set(
        &state,
        &tenant,
        &json!({
            "target": {"doc_prefix": "hr/"},
            "rule": [{"column_or_attr": "role", "op": "eq", "value": {"literal": "hr"}}],
        }),
        None,
    )
    .await
    .expect("policy set ok");

    let end_user = dave();
    let result = docs::doc_search(
        &state,
        &tenant,
        &json!({"query": "salarybandalpha", "k": 5, "mode": "embeddings"}),
        Some(&end_user),
    )
    .await
    .expect("search ok");
    assert_eq!(result["index"]["mode"], json!("embeddings"), "{result:?}");
    assert!(
        result["results"].as_array().expect("results array").is_empty(),
        "a user without the hr role must get zero hits on an hr-only term: {result:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn no_role_gets_zero_hits_in_hybrid_mode() {
    let dir = scratch_dir("hybrid");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "rowpol-ac05-hybrid").await;

    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).expect("valid JSON body");
            let inputs = body["input"].as_array().expect("input array");
            let data: Vec<serde_json::Value> =
                inputs.iter().map(|_| json!({"embedding": [1.0, 0.0]})).collect();
            ResponseTemplate::new(200).set_body_json(json!({"data": data}))
        })
        .mount(&provider)
        .await;
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

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "hr/comp.md", "content": "the salarybandalpha applies to managers only"}),
    )
    .await
    .expect("put hr doc ok");
    docs_index::tick_once(&state).await.expect("tick ok");

    rowpolicy::policy_set(
        &state,
        &tenant,
        &json!({
            "target": {"doc_prefix": "hr/"},
            "rule": [{"column_or_attr": "role", "op": "eq", "value": {"literal": "hr"}}],
        }),
        None,
    )
    .await
    .expect("policy set ok");

    let end_user = dave();
    let result = docs::doc_search(
        &state,
        &tenant,
        &json!({"query": "salarybandalpha", "k": 5, "mode": "hybrid"}),
        Some(&end_user),
    )
    .await
    .expect("search ok");
    assert_eq!(result["index"]["mode"], json!("hybrid"), "{result:?}");
    assert!(
        result["results"].as_array().expect("results array").is_empty(),
        "a user without the hr role must get zero hits on an hr-only term: {result:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
