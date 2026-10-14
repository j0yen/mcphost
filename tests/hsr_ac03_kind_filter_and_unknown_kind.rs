//! PRD-mcphost-help-search-tool
//! AC3 (P0) — Given `q: "prev result"` with `kind: "chain"`, When searched,
//! Then every hit's `anchor` starts with `docs/kinds/chain.md#`; Given
//! `kind: "webhok"`, Then the error is `args_invalid` and its message lists
//! the registered kinds.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::kinds::{KindRegistry, chain::ChainKind};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

async fn server() -> TestServer {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(ChainKind));
    TestServer::start_full(Some(common::ADMIN_KEY.to_string()), kinds, Duration::from_secs(10), None).await
}

#[tokio::test]
async fn kind_chain_restricts_every_hit_to_the_chain_page() {
    let server = server().await;
    let client = McpClient::new(&server.base_url);

    let result = client
        .tools_call("host.help.search", json!({"q": "prev result", "kind": "chain"}))
        .await
        .expect("host.help.search with a registered kind");
    let body = extract_structured(&result);
    let hits = body["hits"].as_array().expect("hits array");
    assert!(!hits.is_empty(), "prev result has hits on the chain page: {body}");
    for hit in hits {
        let anchor = hit["anchor"].as_str().expect("anchor");
        assert!(anchor.starts_with("docs/kinds/chain.md#"), "{anchor} is not on the chain page: {body}");
    }
}

#[tokio::test]
async fn unknown_kind_is_args_invalid_naming_the_registered_kinds() {
    let server = server().await;
    let client = McpClient::new(&server.base_url);

    let err = client
        .tools_call("host.help.search", json!({"q": "prev result", "kind": "webhok"}))
        .await
        .expect_err("an unregistered kind is refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    for kind in ["chain", "echo"] {
        assert!(err.message.contains(kind), "message must list the registered kind {kind}: {}", err.message);
    }
}
