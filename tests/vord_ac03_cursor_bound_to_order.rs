//! PRD-mcphost-declared-ordering-per-verb
//! AC3 (P0) -- Given a cursor minted under `created_desc`, When passed with
//! `order: "name_asc"`, Then `cursor_invalid {reason: "order"}`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup_and_make_pro};
use serde_json::{Value, json};

async fn first_page(client: &McpClient, order: &str) -> Value {
    extract_structured(
        &client
            .tools_call("host.docs.list", json!({"limit": 2, "order": order}))
            .await
            .expect("first page"),
    )
}

#[tokio::test]
async fn cursor_minted_under_created_desc_is_refused_under_name_asc() {
    let server = TestServer::start().await;
    let (_ns, key, _id) = signup_and_make_pro(&server, "Vord AC3", "cus_vord_ac3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for name in ["b", "d", "a", "c", "e"] {
        client.tools_call("host.docs.put", json!({"name": name, "content": "x"})).await.expect("put");
    }

    let page = first_page(&client, "created_desc").await;
    let cursor = page["next_cursor"].as_str().expect("next_cursor").to_string();

    let err = client
        .tools_call("host.docs.list", json!({"limit": 2, "order": "name_asc", "cursor": cursor}))
        .await
        .expect_err("a created_desc cursor must not resume a name_asc walk");
    assert_eq!(err.error_code.as_deref(), Some("cursor_invalid"), "{err:?}");
    assert_eq!(err.data["reason"], json!("order"), "{err:?}");

    // The same cursor with its own order (spelled out, and as the default)
    // still resumes the walk.
    for args in [
        json!({"limit": 2, "order": "created_desc", "cursor": cursor}),
        json!({"limit": 2, "cursor": cursor}),
    ] {
        let next = extract_structured(&client.tools_call("host.docs.list", args).await.expect("same order"));
        assert_eq!(next["documents"].as_array().expect("documents").len(), 2, "{next}");
    }

    // And the other direction: a name_asc cursor is refused under the default.
    let named = first_page(&client, "name_asc").await;
    let named_cursor = named["next_cursor"].as_str().expect("next_cursor");
    let err = client
        .tools_call("host.docs.list", json!({"limit": 2, "cursor": named_cursor}))
        .await
        .expect_err("a name_asc cursor must not resume a created_desc walk");
    assert_eq!(err.data["reason"], json!("order"), "{err:?}");
}

#[tokio::test]
async fn an_undeclared_order_is_args_invalid() {
    let server = TestServer::start().await;
    let (_ns, key, _id) = signup_and_make_pro(&server, "Vord AC3b", "cus_vord_ac3b").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let err = client
        .tools_call("host.docs.list", json!({"order": "random_asc"}))
        .await
        .expect_err("unknown order");
    assert!(err.message.contains("name_asc"), "the refusal lists the declared orders: {err:?}");
}
