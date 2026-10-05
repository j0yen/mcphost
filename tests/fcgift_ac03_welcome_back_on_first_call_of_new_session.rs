//! PRD-mcphost-first-call-gift
//! AC3 — Given a tenant with one note and a completed earlier session,
//! When the first `host.*` call of a new URL-bound session succeeds, Then
//! the result carries `welcome_back` with `notes = 1`, `last_note` equal
//! to the note text, and `last_seen` within the earlier session's window.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn new_url_bound_session_sees_welcome_back_from_the_earlier_session() {
    let server = TestServer::start().await;

    // First (earlier) session: signs up with a first-contact note, makes
    // at least one authenticated call (so `last_seen_unix` is actually
    // set), then mints this tenant's personal URL.
    let session1 = McpClient::new(&server.base_url).with_session_continuity();
    session1
        .tools_call("signup", json!({"name": "AC3 Tenant", "remember": "cleaning Joe's CSVs"}))
        .await
        .expect("signup with remember");
    session1
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami on the earlier session");
    let rotated = extract_structured(
        &session1
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a personal URL"),
    );
    let url = rotated["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();
    let before_new_session_unix = mcphost::state::now_unix();

    // Second, brand-new, URL-bound session: its first host.* call must
    // carry welcome_back.
    let session2 = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first_call = extract_structured(
        &session2
            .tools_call("host.whoami", json!({}))
            .await
            .expect("first call of the new URL-bound session"),
    );

    let welcome_back = &first_call["welcome_back"];
    assert_eq!(welcome_back["notes"], json!(1), "{first_call}");
    assert_eq!(welcome_back["last_note"], json!("cleaning Joe's CSVs"), "{first_call}");
    // `rfc3339_from_unix`'s fixed "YYYY-MM-DDTHH:MM:SSZ" format sorts
    // lexicographically the same as the unix seconds it was built from, so
    // a plain string comparison against a timestamp taken just before the
    // new session's own first call proves `last_seen` came from the
    // earlier session, with no RFC3339 parser needed in this test crate.
    let last_seen_str = welcome_back["last_seen"].as_str().expect("last_seen string");
    let before_new_session_str = mcphost::state::rfc3339_from_unix(before_new_session_unix);
    assert!(
        last_seen_str <= before_new_session_str.as_str(),
        "last_seen ({last_seen_str}) must be from the earlier session, before the new one \
         started ({before_new_session_str})"
    );
}
