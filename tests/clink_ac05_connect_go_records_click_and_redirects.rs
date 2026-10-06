//! AC5 (PRD-mcphost-client-install-links) — Given a click on
//! `/connect/go/cursor` from a non-fleet IP, When the server handles it,
//! Then an `install_link` event with `client=cursor` and `origin=human`
//! is recorded and the response is a 302 to the Cursor deep link.

use crate::common;
use common::TestServer;
use mcphost::install_links;

#[tokio::test]
async fn clicking_connect_go_cursor_records_human_click_and_redirects_to_the_deep_link() {
    let server = TestServer::start().await;
    let links = install_links::for_url(&server.state.public_url);

    let before = server
        .state
        .db
        .install_link_click_counts_by_client(0)
        .await
        .expect("install_link_click_counts_by_client");
    let before_cursor = before.iter().find(|(c, _)| c == "cursor").map(|(_, n)| *n).unwrap_or(0);

    // The test harness's own loopback connection has no fleet IP
    // configured (`TestServer::start()`'s default empty fleet list), so
    // this request classifies as `human` the same way any real visitor's
    // browser would.
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
        .get(format!("{}/connect/go/cursor", server.base_url))
        .send()
        .await
        .expect("GET /connect/go/cursor");

    assert_eq!(response.status(), 302, "must be a 302 Found, not axum's default 303");
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(location, links.cursor, "302 must point at the Cursor deep link");

    let after = server
        .state
        .db
        .install_link_click_counts_by_client(0)
        .await
        .expect("install_link_click_counts_by_client");
    let after_cursor = after.iter().find(|(c, _)| c == "cursor").map(|(_, n)| *n).unwrap_or(0);
    assert_eq!(
        after_cursor,
        before_cursor + 1,
        "exactly one human cursor click must be recorded: before={before:?} after={after:?}"
    );
}
