//! PRD-mcphost-help-search-tool
//! AC4 (P0) — Given any `q` from the AC1 fixture with `k=3`, When
//! serialized, Then the response is <= 2048 bytes and a truncated example is
//! marked `truncated: true`.

use crate::common;
use crate::hsr_ac01_top10_errors_have_fix_anchor as ac01;
use common::{McpClient, TestServer, extract_structured};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

#[tokio::test]
async fn every_fixture_query_is_under_2kb_and_cut_examples_are_marked() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let index = mcphost::help_search::index(&KindRegistry::with_builtin());

    let mut cut_examples = 0;
    // The ten fixture errors at k=3 (the budgeted default), plus the queries
    // whose hits carry the longest examples, widened to k=5, which is where
    // the cut has to engage.
    let mut queries: Vec<(String, usize)> = ac01::fixture().into_iter().map(|(error, _)| (error, 3)).collect();
    queries.push(("import mcphost.dev".to_string(), 3));
    queries.push(("import mcphost.dev".to_string(), 5));
    queries.push(("python source main args mcphost".to_string(), 5));
    for (q, k) in &queries {
        let result = client
            .tools_call("host.help.search", json!({"q": q, "k": k}))
            .await
            .unwrap_or_else(|e| panic!("host.help.search '{q}' failed: {} {}", e.code, e.message));
        let body = extract_structured(&result);
        let size = serde_json::to_string(&body).expect("serializes").len();
        assert!(size <= 2048, "'{q}' answered {size} bytes (> 2048): {body}");

        let (full, _) = index.search(q, *k, None);
        let served = body["hits"].as_array().expect("hits");
        for (full_hit, hit) in full.iter().zip(served) {
            let Some(full_ex) = &full_hit.example else { continue };
            let code = hit["example"]["code"].as_str().unwrap_or_default();
            let marked = hit["example"]["truncated"] == Value::Bool(true);
            if code == full_ex.code {
                assert!(!marked, "'{q}': an intact example must not claim truncated: {hit}");
            } else {
                cut_examples += 1;
                assert!(marked, "'{q}': example was cut but not marked truncated: true: {hit}");
                assert!(full_ex.code.starts_with(code), "'{q}': a cut example is a prefix of the full one: {hit}");
            }
        }
    }
    assert!(cut_examples > 0, "no query exercised truncation; the 2 KB budget is untested");
}

#[tokio::test]
async fn an_oversized_k5_answer_is_still_cut_to_the_budget() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);
    let result = client
        .tools_call("host.help.search", json!({"q": "python source main args mcphost", "k": 5}))
        .await
        .expect("host.help.search k=5");
    let body = extract_structured(&result);
    let size = serde_json::to_string(&body).expect("serializes").len();
    assert!(size <= 2048, "k=5 answered {size} bytes (> 2048): {body}");
}
