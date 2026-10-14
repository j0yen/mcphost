//! PRD-mcphost-feedback-closed-loop-notice AC3 — Given the same theme, When
//! `admin.feedback.ship` runs a second time, Then it returns `notified: 0,
//! already_notified: 3` and no new message row exists.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, fbcl_themed};
use serde_json::json;

#[tokio::test]
async fn second_ship_notifies_no_one_and_adds_no_message_row() {
    let server = TestServer::start().await;
    let (clients, _ids, theme_id) = fbcl_themed(&server, &["fbcl3-a", "fbcl3-b", "fbcl3-c"], 2).await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let args = json!({"theme_id": theme_id, "version": "0.110.0", "anchor": "args"});
    let first = extract_structured(&admin.tools_call("admin.feedback.ship", args.clone()).await.unwrap());
    assert_eq!(first["notified"], 3, "{first}");

    let count_messages = || async {
        let mut n = 0;
        for c in &clients {
            let inbox = extract_structured(&c.tools_call("host.msg.inbox", json!({})).await.unwrap());
            n += inbox["messages"].as_array().unwrap().len();
        }
        n
    };
    assert_eq!(count_messages().await, 3);

    // A later call even with a different version must not re-notify.
    let second = extract_structured(
        &admin
            .tools_call("admin.feedback.ship", json!({"theme_id": theme_id, "version": "0.111.0"}))
            .await
            .unwrap(),
    );
    assert_eq!(second["notified"], 0, "{second}");
    assert_eq!(second["already_notified"], 3, "{second}");
    assert_eq!(count_messages().await, 3, "no new message row");
}
