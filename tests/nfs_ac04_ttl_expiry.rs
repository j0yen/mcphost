//! PRD-mcphost-notes-to-future-self AC4 (P0) -- Given a note with
//! `ttl_s: 60` and the clock advanced 61 s, When `host.note.list` and
//! `tools/list` run before the sweep, Then neither shows it; When the sweep
//! runs, Then the row is gone.

use crate::common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn expired_note_is_hidden_before_the_sweep_and_deleted_by_it() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "NFS AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let qualified = publish(&client, "cleaner", "echo", json!({"schema": {"type": "object"}})).await;
    let put = extract_structured(
        &client
            .tools_call(
                "host.note.put",
                json!({"text": "short lived", "attach_to": {"tool": "cleaner"}, "ttl_s": 60}),
            )
            .await
            .expect("put"),
    );
    assert!(put["expires_at"].is_string(), "{put}");
    let note_key = put["key"].as_str().expect("key").to_string();

    let described = |listed: &serde_json::Value| -> String {
        listed["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|t| t["name"] == qualified.as_str())
            .expect("cleaner listed")["description"]
            .as_str()
            .expect("description")
            .to_string()
    };
    let listed = client.tools_list().await.expect("tools/list");
    assert!(described(&listed).contains("short lived"), "visible before expiry");

    // The blob clock is the injected test clock every TTL feature shares.
    server.state.blob_clock_offset_secs.store(61, Ordering::Relaxed);

    let notes = extract_structured(&client.tools_call("host.note.list", json!({})).await.expect("list"));
    assert_eq!(notes["notes"], json!([]), "expired note is hidden from host.note.list");
    let listed = client.tools_list().await.expect("tools/list");
    assert!(!described(&listed).contains("note to self"), "expired note is hidden from tools/list");
    let tenant = server.state.db.find_tenant_by_key_hash(mcphost::auth::hash_key(&key)).await.expect("tenant").expect("some");
    assert!(
        server.state.db.state_kv_get(tenant.id, note_key.clone(), String::new()).await.expect("get").is_some(),
        "the row is still stored before the sweep"
    );

    let swept = server.state.db.prune_expired_notes(mcphost::notes::now(&server.state)).await.expect("sweep");
    assert_eq!(swept, 1);
    assert!(
        server.state.db.state_kv_get(tenant.id, note_key, String::new()).await.expect("get").is_none(),
        "the sweep removed the row"
    );
}
