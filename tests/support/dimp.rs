//! PRD-mcphost-docs-import-and-stated-boundary: shared fixture for the dimp
//! tests (a signed-up client, `put_many` and `docs.list` readers).
//!
//! Not a crate module: pulled in per-file with `#[path = "support/dimp.rs"]
//! mod dimp;`, same convention as `tests/support/tcpn.rs`.

#![allow(dead_code)]

use crate::common;
use common::{McpClient, RpcError, TestServer, extract_structured, signup};
use serde_json::{Value, json};

/// A client bound to a freshly signed-up tenant of `server`.
pub async fn tenant(server: &TestServer, name: &str) -> McpClient {
    let (_ns, key) = signup(&server.base_url, name).await;
    McpClient::with_bearer(&server.base_url, &key)
}

/// `n` text documents `doc-00.md`, `doc-01.md`, ...; every odd one is sent as
/// `content_base64`, so a batch mixes both import paths.
pub fn documents(n: usize) -> Vec<Value> {
    use base64::Engine as _;
    (0..n)
        .map(|i| {
            let name = format!("doc-{i:02}.md");
            let body = format!("# Document {i}\n\nThe marker word for this one is dimpword{i}.\n");
            if i % 2 == 1 {
                let b64 = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
                json!({"name": name, "content_base64": b64})
            } else {
                json!({"name": name, "content": body})
            }
        })
        .collect()
}

/// `host.docs.put_many`; the raw `Result` so a test can read a JSON-RPC error.
pub async fn put_many_raw(client: &McpClient, documents: Vec<Value>, on_conflict: Option<&str>) -> Result<Value, RpcError> {
    let mut args = json!({"documents": documents});
    if let Some(mode) = on_conflict {
        args["on_conflict"] = json!(mode);
    }
    client.tools_call("host.docs.put_many", args).await
}

/// `host.docs.put_many` that must be a JSON-RPC success, answering its structured result.
pub async fn put_many(client: &McpClient, documents: Vec<Value>, on_conflict: Option<&str>) -> Value {
    let reply = put_many_raw(client, documents, on_conflict).await.expect("put_many is a JSON-RPC success");
    extract_structured(&reply)
}

/// Every live document name `host.docs.list` returns, walking its pages.
pub async fn listed_names(client: &McpClient) -> Vec<String> {
    let mut names = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = json!({"limit": 25});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let page = extract_structured(&client.tools_call("host.docs.list", args).await.expect("list ok"));
        names.extend(page["documents"].as_array().expect("documents array").iter().map(|d| d["name"].as_str().expect("name").to_string()));
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => return names,
        }
    }
}

/// The text `host.docs.get` reads back for `name`.
pub async fn text_of(client: &McpClient, name: &str) -> String {
    let doc = extract_structured(&client.tools_call("host.docs.get", json!({"name": name, "text": true})).await.expect("get ok"));
    doc["text"].as_str().expect("text").to_string()
}
