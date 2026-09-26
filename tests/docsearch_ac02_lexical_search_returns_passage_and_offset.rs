//! PRD-mcphost-docs-semantic-search
//! AC2 -- Given a document containing the phrase "refunds are processed
//! within 14 days" in its third paragraph, When `search {query: "refund
//! window", k: 3}` runs in lexical mode, Then the top result names that
//! document with the passage text and an `offset` inside the third
//! paragraph.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac02-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn top_result_names_the_document_with_passage_and_offset_in_third_paragraph() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docsearch-ac02").await;

    // Two long filler paragraphs (~750 chars each) so the third, short
    // paragraph can't fit in either of their chunks (the 800-char budget)
    // and opens its own chunk instead -- its chunk's `offset` therefore
    // lands exactly at its own start.
    let filler_one = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ".repeat(12);
    let filler_two = "kilo lima mike november oscar papa quebec romeo sierra tango ".repeat(13);
    let third_paragraph = "Refunds are processed within 14 days of purchase, no exceptions.";
    let content = format!("{filler_one}\n\n{filler_two}\n\n{third_paragraph}");
    let third_paragraph_offset = content.find(third_paragraph).expect("phrase present") as i64;

    let put = docs::doc_put(&state, &tenant, &json!({"name": "refund-policy.md", "content": content}))
        .await
        .expect("put ok");
    let document_id = put["id"].as_str().expect("id").to_string();
    docs_index::tick_once(&state).await.expect("tick ok");

    let result =
        docs::doc_search(&state, &tenant, &json!({"query": "refund window", "k": 3}))
            .await
            .expect("search ok");
    assert_eq!(result["index"]["mode"], json!("lexical"));

    let results = result["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "expected at least one result: {result:?}");
    let top = &results[0];
    assert_eq!(top["document_id"], json!(document_id), "top result: {top:?}");
    assert_eq!(top["name"], json!("refund-policy.md"));
    let top_text = top["text"].as_str().expect("text");
    assert!(top_text.contains(third_paragraph), "top result text must contain the passage: {top_text}");
    let offset = top["offset"].as_i64().expect("offset");
    assert!(
        offset >= third_paragraph_offset && offset < third_paragraph_offset + third_paragraph.len() as i64,
        "offset {offset} must fall inside the third paragraph starting at {third_paragraph_offset}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
