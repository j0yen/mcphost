//! PRD-mcphost-url-bound-tenants
//! AC4 (P0) — Given a browser, When `GET /u/new` then the button's
//! `POST /u/new` run, Then a tenant exists with `source: "url-page"`, the
//! page shows the URL and four copy snippets, and a sixth `POST` from the
//! same IP within the hour is refused with the signup limiter's error.

#[tokio::test]
async fn get_new_url_serves_a_one_button_page() {
    let server = crate::common::TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .get(format!("{}/u/new", server.base_url))
        .send()
        .await
        .expect("GET /u/new");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("read body");
    assert!(body.contains("<form"), "the page must have exactly one button/form: {body}");
    assert!(body.contains("action=\"/u/new\""), "the form must POST back to /u/new: {body}");
    assert!(body.to_lowercase().contains("<button"), "the page must have a button: {body}");
}

#[tokio::test]
async fn post_new_url_mints_a_tenant_and_shows_four_snippets() {
    let server = crate::common::TestServer::start().await;
    let http = reqwest::Client::new();

    let before = server.state.db.list_tenants().await.unwrap().len();

    let resp = http
        .post(format!("{}/u/new", server.base_url))
        .send()
        .await
        .expect("POST /u/new");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("read body");

    let after = server.state.db.list_tenants().await.unwrap();
    assert_eq!(after.len(), before + 1, "POST /u/new must mint exactly one tenant");
    let minted = after
        .iter()
        .max_by_key(|t| t.id)
        .expect("the just-minted tenant");
    assert_eq!(
        minted.signup_source.as_deref(),
        Some("url-page"),
        "the minted tenant must be stamped source: url-page"
    );
    assert!(
        minted.url_secret_hash.is_some(),
        "the minted tenant must already have a URL secret"
    );

    // The page shows the URL and four copy snippets: Claude Code, Claude
    // Desktop/claude.ai (paste), Cursor (JSON), and generic JSON.
    assert!(body.contains(&format!("{}/u/", server.base_url)), "page must show the URL: {body}");
    assert!(body.contains("claude mcp add"), "Claude Code snippet missing: {body}");
    assert!(body.to_lowercase().contains("claude desktop") || body.to_lowercase().contains("claude.ai"), "Claude Desktop/claude.ai snippet missing: {body}");
    assert!(body.to_lowercase().contains("cursor"), "Cursor snippet missing: {body}");
    assert!(body.contains("mcpServers") || body.contains("&quot;url&quot;"), "generic JSON snippet missing: {body}");
}

#[tokio::test]
async fn sixth_post_from_the_same_ip_in_an_hour_is_refused() {
    let server = crate::common::TestServer::start().await;
    let http = reqwest::Client::new();
    let limit = server.state.signup_rate_limit_per_hour;

    for i in 0..limit {
        let resp = http
            .post(format!("{}/u/new", server.base_url))
            .send()
            .await
            .unwrap_or_else(|e| panic!("POST /u/new {i} should succeed: {e:?}"));
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "POST /u/new {i} should succeed");
    }

    let before = server.state.db.list_tenants().await.unwrap().len();
    let resp = http
        .post(format!("{}/u/new", server.base_url))
        .send()
        .await
        .expect("one too many POST /u/new");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "past the signup limiter's own cap, /u/new must refuse the same way signup does"
    );
    let after = server.state.db.list_tenants().await.unwrap().len();
    assert_eq!(after, before, "the rate-limited POST must not mint a tenant");
}
