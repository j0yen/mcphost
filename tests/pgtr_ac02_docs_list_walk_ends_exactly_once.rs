//! PRD-mcphost-paged-trait-on-every-list-verb
//! AC2 (P0) -- Given a tenant with 450 documents, When an agent walks
//! `host.docs.list(limit=200)` following `next_cursor`, Then it makes exactly
//! 3 calls, the third response has no `next_cursor` key, and the union is 450
//! distinct names.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn docs_list_walk_of_450_documents_takes_three_calls_and_ends_without_a_cursor() {
    let server = TestServer::start().await;
    let (_ns, key, _id) = signup_and_make_pro(&server, "Pgtr AC2", "cus_pgtr_ac2").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for i in 0..450 {
        client
            .tools_call("host.docs.put", json!({"name": format!("doc-{i:03}"), "content": "x"}))
            .await
            .unwrap_or_else(|e| panic!("put doc-{i:03}: {} {}", e.code, e.message));
    }

    let mut names = BTreeSet::new();
    let mut calls = 0;
    let mut cursor: Option<String> = None;
    let last = loop {
        let mut args = json!({"limit": 200});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let page = extract_structured(&client.tools_call("host.docs.list", args).await.expect("list"));
        calls += 1;
        for d in page["documents"].as_array().expect("documents array") {
            names.insert(d["name"].as_str().expect("name").to_string());
        }
        match page.get("next_cursor") {
            Some(c) => cursor = Some(c.as_str().expect("next_cursor is a string").to_string()),
            None => break page,
        }
        assert!(calls < 10, "walk did not terminate");
    };

    assert_eq!(calls, 3, "exactly ceil(450/200) calls");
    assert!(last.get("next_cursor").is_none(), "last page must omit the key, not null it: {last:?}");
    assert_eq!(names.len(), 450, "union of pages is every document exactly once");
}
