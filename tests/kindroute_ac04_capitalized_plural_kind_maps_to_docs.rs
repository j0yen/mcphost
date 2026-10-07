//! PRD-mcphost-kind-ask-routing
//! AC4 (P0) -- Given `kind: Documents` (capitalized, plural), When matched,
//! Then it maps to `docs`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, all_kinds_registry, signup};
use mcphost::kinds::outcomes::find;
use serde_json::json;

#[test]
fn normalization_is_case_insensitive_and_singular_plural() {
    for word in ["Documents", "DOCS", "document", " Docs ", "Docs"] {
        assert_eq!(find(word).map(|o| o.outcome), Some("docs"), "word {word:?}");
    }
    // Plural of a listed singular matches without being listed.
    assert_eq!(find("Databases").map(|o| o.outcome), Some("database"));
    // No fuzzy matching.
    assert!(find("documnets").is_none());
    assert!(find("lambda").is_none());
}

#[tokio::test]
async fn tool_publish_kind_documents_answers_with_the_docs_outcome() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "Kindroute Ask AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "my_tool", "kind": "Documents", "spec": {}}),
        )
        .await
        .expect_err("Documents is an outcome word");
    assert!(err.message.contains("kind 'Documents' is not registered"), "{}", err.message);
    assert_eq!(err.data["did_you_mean"]["outcome"], json!("docs"));
}
