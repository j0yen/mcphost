//! PRD-mcphost-notes-to-future-self-b AC2 (P0) -- Given tool `t` owned by A
//! and shared to B, When B lists tools, Then B's entry for `A.tool_t` carries no
//! suffix; Given A and B both attach notes to their own tools concurrently,
//! Then each sees only its own.

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use serde_json::json;

const MARK: &str = " — note to self: ";

async fn descriptions(client: &McpClient) -> Vec<(String, String)> {
    let listed = client.tools_list().await.expect("tools/list");
    listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap_or_default().to_string(),
                t["description"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn nfsb_ac02() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "NFSB A").await;
    let (_ns_b, key_b) = signup(&server.base_url, "NFSB B").await;
    let a = McpClient::with_bearer(&server.base_url, &key_a);
    let b = McpClient::with_bearer(&server.base_url, &key_b);

    let qual_a = publish(&a, "tool_t", "echo", json!({"schema": {"type": "object"}})).await;
    a.tools_call("host.tool_share", json!({"name": "tool_t", "visibility": "public"}))
        .await
        .expect("share t");
    let qual_b = publish(&b, "tool_u", "echo", json!({"schema": {"type": "object"}})).await;

    let (ra, rb) = tokio::join!(
        a.tools_call("host.note.put", json!({"text": "A-secret-note", "attach_to": {"tool": "tool_t"}})),
        b.tools_call("host.note.put", json!({"text": "B-private-note", "attach_to": {"tool": "tool_u"}})),
    );
    ra.expect("A put");
    rb.expect("B put");

    let a_view = descriptions(&a).await;
    let b_view = descriptions(&b).await;

    let a_t = a_view.iter().find(|(n, _)| *n == qual_a).expect("A lists A.tool_t");
    assert!(a_t.1.ends_with(&format!("{MARK}A-secret-note")), "{a_t:?}");
    assert!(!a_view.iter().any(|(_, d)| d.contains("B-private-note")), "{a_view:?}");

    let b_u = b_view.iter().find(|(n, _)| *n == qual_b).expect("B lists B.u");
    assert!(b_u.1.ends_with(&format!("{MARK}B-private-note")), "{b_u:?}");
    // B's entry for the shared `A.tool_t` (if listed at all) carries no suffix.
    for (name, desc) in &b_view {
        assert!(!desc.contains("A-secret-note"), "{name} leaks A's note: {desc}");
        if name == &format!("{ns_a}.tool_t") {
            assert!(!desc.contains(MARK), "{name}: {desc}");
        }
    }
}
