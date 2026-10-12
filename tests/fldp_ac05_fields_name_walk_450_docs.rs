//! PRD-mcphost-fields-projection-on-list-verbs
//! AC5 (P0) -- Given `fields: ["name"]` and a 450-document walk with
//! `limit: 200`, When paged to the end, Then 3 calls return 450 distinct
//! names and the cursor still resumes correctly although `seq` was not
//! requested.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup_and_make_pro};
use serde_json::json;
use std::collections::BTreeSet;

#[tokio::test]
async fn projected_walk_of_450_docs_takes_three_calls_with_the_cursor_intact() {
    let server = TestServer::start().await;
    let (_ns, key, _id) = signup_and_make_pro(&server, "Fldp AC5", "cus_fldp_ac5").await;
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
        let mut args = json!({"fields": ["name"], "limit": 200});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let page = extract_structured(&client.tools_call("host.docs.list", args).await.expect("list"));
        calls += 1;
        for d in page["documents"].as_array().expect("documents array") {
            assert_eq!(d.as_object().expect("row").keys().collect::<Vec<_>>(), ["name"], "{d}");
            names.insert(d["name"].as_str().expect("name").to_string());
        }
        match page.get("next_cursor") {
            Some(c) => cursor = Some(c.as_str().expect("next_cursor is a string").to_string()),
            None => break page,
        }
        assert!(calls < 10, "walk did not terminate");
    };
    assert_eq!(calls, 3, "exactly ceil(450/200) calls");
    assert!(last.get("next_cursor").is_none(), "last page omits the key: {last:?}");
    assert_eq!(names.len(), 450, "every document exactly once, no gap or repeat at a page seam");
}
