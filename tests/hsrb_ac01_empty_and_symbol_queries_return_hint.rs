//! PRD-mcphost-help-search-tool-b
//! AC1 (P0) — Given `q: ""` and `q: "???"`, When searched, Then both return
//! `hits: []`, `total: 0` and the hint, with no error.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn hsrb_ac01_empty_and_symbol_only_queries_return_empty_hits_and_hint() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    for q in ["", "???"] {
        let result = client
            .tools_call("host.help.search", json!({"q": q}))
            .await
            .unwrap_or_else(|e| panic!("q={q:?} must not be an error: {} {}", e.code, e.message));
        let body = extract_structured(&result);
        assert_eq!(body["hits"], json!([]), "q={q:?}: {body}");
        assert_eq!(body["total"], json!(0), "q={q:?}: {body}");
        assert_eq!(
            body["hint"].as_str(),
            Some("try a tool name, an error code or an import line"),
            "q={q:?}: {body}"
        );
    }
}
