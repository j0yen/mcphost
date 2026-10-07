//! PRD-mcphost-ownership-moment
//! AC1 (P0) — Given an implicit first call, a `/u/new` mint, a URL-bound
//! first call, and an invite join, When each response's onboarding
//! envelope is read, Then it carries `claim_url`, and `host.whoami` carries
//! the same URL until the tenant is claimed.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn is_claim_url(url: &str) -> bool {
    url.starts_with("https://") && url.contains("/claim/")
}

/// The `/u/{secret}/mcp` path embedded as a bare token between `<code>`
/// tags on the `/u/new` result page -- same delimiter-split idiom as every
/// other HTML-scraping test in this suite (e.g.
/// `tests/mcphost_human_claim_magic_link_ac01_claim_url_in_signup.rs`'s own
/// shape check), just applied to a page body instead of a JSON field.
fn find_url_containing<'a>(body: &'a str, needle: &str) -> &'a str {
    body.split(['<', '>', '"', ' ', '\n', '\\'])
        .find(|tok| tok.starts_with("http") && tok.contains(needle))
        .unwrap_or_else(|| panic!("no http(s):// url containing '{needle}' in body: {body}"))
}

#[tokio::test]
async fn implicit_first_call_carries_claim_url() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();

    let publish = session
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "hello",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
            }),
        )
        .await
        .expect("bare host.tool_publish must succeed");
    let published = extract_structured(&publish);
    let claim_url = published["onboarding"]["claim_url"]
        .as_str()
        .expect("onboarding.claim_url present on an implicit first call")
        .to_string();
    assert!(is_claim_url(&claim_url), "not a claim url: {claim_url}");

    // PRD-mcphost-session-bound-tenant-key requirement 2 (AC2): a second
    // key-less call on this same session is now refused and named by
    // default, rather than silently served -- so this AC's own "host.whoami
    // on the bound session" is read over the tenant's own `/u/` URL (the
    // `onboarding.url` this very call just minted), its real credential,
    // not the now-refused key-less continuation.
    let claim_url_path = published["onboarding"]["url"]
        .as_str()
        .expect("onboarding.url present")
        .to_string();
    let url_client =
        McpClient::new(&server.base_url).with_path(claim_url_path.trim_start_matches(&server.base_url));
    let whoami = extract_structured(
        &url_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("host.whoami over this tenant's own /u/ URL"),
    );
    assert_eq!(
        whoami["claim_url"].as_str(),
        Some(claim_url.as_str()),
        "host.whoami must carry the SAME claim_url the onboarding envelope minted: {whoami}"
    );
}

#[tokio::test]
async fn u_new_mint_page_carries_a_claim_link() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .post(format!("{}/u/new", server.base_url))
        .send()
        .await
        .expect("POST /u/new");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");

    let claim_url = find_url_containing(&body, "/claim/");
    assert!(is_claim_url(claim_url), "not a claim url: {claim_url}");
}

#[tokio::test]
async fn url_bound_first_call_carries_claim_url_matching_the_mint_page() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let resp = http
        .post(format!("{}/u/new", server.base_url))
        .send()
        .await
        .expect("POST /u/new");
    let body = resp.text().await.expect("body");
    let minted_url = find_url_containing(&body, "/u/").to_string();
    let minted_claim_url = find_url_containing(&body, "/claim/").to_string();
    let path = minted_url.trim_start_matches(&server.base_url).to_string();

    // When: this tenant's first-ever authenticated call, over its own
    // `/u/{secret}/mcp` link -- no Authorization header, no tenant_key.
    let url_client = McpClient::new(&server.base_url).with_path(&path);
    let first = extract_structured(
        &url_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("the URL-bound tenant's first call must succeed"),
    );
    let onboarding_claim_url = first["onboarding"]["claim_url"]
        .as_str()
        .expect("onboarding.claim_url present on the URL-bound first call");
    assert_eq!(
        onboarding_claim_url, minted_claim_url,
        "the URL-bound first call's onboarding.claim_url must be the SAME link /u/new minted"
    );
    assert_eq!(
        first["claim_url"].as_str(),
        Some(minted_claim_url.as_str()),
        "host.whoami's own claim_url must agree too: {first}"
    );

    // A second call on the same URL is no longer the tenant's "first" one
    // -- no onboarding envelope this time.
    let second = extract_structured(
        &url_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("second call over the same URL"),
    );
    assert!(
        second.get("onboarding").is_none(),
        "a later call over the same /u/ URL must not carry onboarding again: {second}"
    );
    // ...but host.whoami keeps reporting the same claim_url regardless.
    assert_eq!(second["claim_url"].as_str(), Some(minted_claim_url.as_str()));
}

#[tokio::test]
async fn invite_join_carries_claim_url() {
    let server = TestServer::start().await;
    let (a_ns, a_key) = signup(&server.base_url, "Inviter A").await;
    let a_client = McpClient::with_bearer(&server.base_url, &a_key);

    a_client
        .tools_call(
            "host.tool_publish",
            json!({
                "name": "mytool",
                "kind": "echo",
                "spec": {"schema": {"type": "object", "properties": {}, "required": []}},
            }),
        )
        .await
        .expect("publish should succeed");
    let created = extract_structured(
        &a_client
            .tools_call("host.invite.create", json!({"share": ["mytool"]}))
            .await
            .expect("invite create should succeed"),
    );
    let url = created["url"].as_str().unwrap().to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let b_session = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first = extract_structured(
        &b_session
            .tools_call("host.tool.call", json!({"name": format!("{a_ns}.mytool")}))
            .await
            .expect("the first call on the invite path must succeed"),
    );
    let claim_url = first["onboarding"]["claim_url"]
        .as_str()
        .expect("onboarding.claim_url present on an invite join")
        .to_string();
    assert!(is_claim_url(&claim_url), "not a claim url: {claim_url}");

    let whoami = extract_structured(
        &b_session
            .tools_call("host.whoami", json!({}))
            .await
            .expect("host.whoami on the invitee's own session"),
    );
    assert_eq!(whoami["claim_url"].as_str(), Some(claim_url.as_str()));
}

#[tokio::test]
async fn whoami_drops_claim_url_and_gains_owner_once_claimed() {
    use std::sync::Arc;

    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let raw = anon
        .tools_call("signup", json!({"name": "AC1 Claim Tenant"}))
        .await
        .expect("signup");
    let result = extract_structured(&raw);
    let key = result["key"].as_str().expect("key").to_string();
    let claim_url = result["claim_url"].as_str().expect("claim_url").to_string();
    let token = claim_url.rsplit('/').next().unwrap().to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let before = extract_structured(
        &tenant_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("host.whoami before claim"),
    );
    assert_eq!(before["claim_url"].as_str(), Some(claim_url.as_str()));
    assert!(before.get("owner").is_none(), "unclaimed tenant must carry no owner block: {before}");

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "owner@example.com")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let body = &fake.sends()[0].text_body;
    let idx = body.find("/claim/verify/").expect("verify URL in email body");
    let code = body[idx + "/claim/verify/".len()..]
        .split_whitespace()
        .next()
        .expect("code token");
    http.get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");

    let after = extract_structured(
        &tenant_client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("host.whoami after claim"),
    );
    assert!(
        after.get("claim_url").is_none(),
        "a claimed tenant must carry no claim_url: {after}"
    );
    assert!(
        after["owner"]["verified_at"].as_i64().is_some(),
        "a claimed tenant must carry owner.verified_at: {after}"
    );
}
