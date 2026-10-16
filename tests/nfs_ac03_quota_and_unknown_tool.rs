//! PRD-mcphost-notes-to-future-self AC3 (P0) -- Given a tenant at
//! `notes_max`, When `host.note.put` is called, Then `quota_exceeded` with
//! `{limit, used}` and no row is written; Given `attach_to: {tool: "nope"}`,
//! Then `not_found`.

use crate::common::{McpClient, TestServer, extract_structured, publish, signup};
use serde_json::json;

#[tokio::test]
async fn put_at_notes_max_is_quota_exceeded_and_unknown_tool_is_not_found() {
    let server = TestServer::start_with_plans(|catalog| {
        for p in &mut catalog.plans {
            if p.name == "free" {
                p.notes_max = 2;
            }
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "NFS AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    publish(&client, "cleaner", "echo", json!({"schema": {"type": "object"}})).await;

    for text in ["one", "two"] {
        client
            .tools_call("host.note.put", json!({"text": text, "attach_to": {"tool": "cleaner"}}))
            .await
            .expect("put under the limit");
    }
    let err = client
        .tools_call("host.note.put", json!({"text": "three", "attach_to": {"tool": "cleaner"}}))
        .await
        .expect_err("third put is over notes_max");
    assert_eq!(err.error_code.as_deref(), Some("quota_exceeded"), "{err:?}");
    assert_eq!(err.data["limit"], 2, "{:?}", err.data);
    assert_eq!(err.data["used"], 2, "{:?}", err.data);
    let listed = extract_structured(&client.tools_call("host.note.list", json!({})).await.expect("list"));
    assert_eq!(listed["notes"].as_array().expect("notes").len(), 2, "no row was written");

    let err = client
        .tools_call("host.note.put", json!({"text": "x", "attach_to": {"tool": "nope"}}))
        .await
        .expect_err("unknown tool");
    assert_eq!(err.error_code.as_deref(), Some("not_found"), "{err:?}");

    // The default plans carry the PRD's numbers.
    assert_eq!(server.state.plans.get("pro").expect("pro").notes_max, 5000);
}
