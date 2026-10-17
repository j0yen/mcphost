//! PRD-mcphost-same-user-second-agent
//! AC6 — Given a browser that consented once (cookie set), When it opens a
//! new authorize request, Then the consent page's first option is "Join
//! <display_name>" and choosing it issues tokens for that tenant with a new
//! `tenant_clients` row and sends zero emails.

use crate::common;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{McpClient, TestServer, extract_structured};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn code_challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

struct Flow {
    http: reqwest::Client,
    base: String,
    client_id: String,
    challenge: String,
    resource: String,
}

const VERIFIER: &str = "a-pkce-verifier-at-least-43-chars-long-for-realism";

impl Flow {
    fn params(&self) -> Vec<(&'static str, String)> {
        vec![
            ("response_type", "code".into()),
            ("client_id", self.client_id.clone()),
            ("redirect_uri", "http://127.0.0.1/cb".into()),
            ("code_challenge", self.challenge.clone()),
            ("code_challenge_method", "S256".into()),
            ("state", "abc".into()),
            ("scope", "mcp".into()),
            ("resource", self.resource.clone()),
        ]
    }

    async fn page(&self, cookie: Option<&str>) -> String {
        let mut req = self.http.get(format!("{}/oauth/authorize", self.base)).query(&self.params());
        if let Some(c) = cookie {
            req = req.header("Cookie", c);
        }
        req.send().await.expect("GET /oauth/authorize").text().await.expect("page")
    }

    async fn consent(&self, extra: (&str, &str), cookie: Option<&str>) -> reqwest::Response {
        let mut form = self.params();
        form.push((extra.0, extra.1.to_string()));
        let mut req = self.http.post(format!("{}/oauth/authorize", self.base)).form(&form);
        if let Some(c) = cookie {
            req = req.header("Cookie", c);
        }
        req.send().await.expect("POST /oauth/authorize")
    }

    async fn token_for(&self, consent: &reqwest::Response) -> String {
        assert!(consent.status().is_redirection(), "consent must redirect: {}", consent.status());
        let location = consent.headers().get("location").expect("Location").to_str().unwrap().to_string();
        let (_, query) = location.split_once('?').expect("query string");
        let code = query_param(query, "code").expect("authorization code issued").to_string();
        let token: Value = self
            .http
            .post(format!("{}/oauth/token", self.base))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", "http://127.0.0.1/cb"),
                ("client_id", self.client_id.as_str()),
                ("code_verifier", VERIFIER),
            ])
            .send()
            .await
            .expect("token exchange")
            .json()
            .await
            .expect("token json");
        token["access_token"].as_str().unwrap_or_else(|| panic!("no access_token: {token}")).to_string()
    }
}

#[tokio::test]
async fn sibl_ac06_returning_browser_is_offered_join() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"], "client_name": "claude-ai"}))
        .send()
        .await
        .expect("register")
        .json()
        .await
        .expect("register json");
    let flow = Flow {
        http,
        base: server.base_url.clone(),
        client_id: register["client_id"].as_str().expect("client_id").to_string(),
        challenge: code_challenge_for(VERIFIER),
        resource: format!("{}/mcp", server.base_url),
    };

    // A first-time browser is not offered a join, and consenting sets the cookie.
    let first_page = flow.page(None).await;
    assert!(!first_page.contains("Join "), "no cookie, no join option: {first_page}");
    let first = flow.consent(("new_workspace", "1"), None).await;
    let set_cookie = first
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("mcphost_consent="))
        .expect("consent sets the cookie")
        .to_string();
    for attr in ["HttpOnly", "Secure", "SameSite=Lax"] {
        assert!(set_cookie.contains(attr), "cookie must carry {attr}: {set_cookie}");
    }
    assert!(set_cookie.contains("Max-Age=2592000"), "30 days: {set_cookie}");
    let cookie = set_cookie.split(';').next().unwrap().to_string();
    let first_token = flow.token_for(&first).await;
    let who = extract_structured(
        &McpClient::with_bearer(&server.base_url, &first_token)
            .tools_call("host.whoami", json!({}))
            .await
            .expect("whoami"),
    );
    let ns = who["namespace"].as_str().expect("namespace").to_string();
    let display = who["display_name"].as_str().expect("display_name").to_string();
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");
    let rows_before = server.state.db.list_live_clients(tenant.id).await.expect("clients").len();

    // The same browser opens a new authorize request: "Join <display_name>" is first.
    let page = flow.page(Some(&cookie)).await;
    let join_at = page.find(&format!("Join {display}")).unwrap_or_else(|| panic!("join option missing: {page}"));
    for later in ["tenant_key", "claim_code", "Create a new workspace"] {
        assert!(join_at < page.find(later).expect(later), "Join must come before {later}: {page}");
    }

    // Choosing it issues a token for the same tenant, a new client row, no email.
    let joined = flow.consent(("join_workspace", "1"), Some(&cookie)).await;
    let second_token = flow.token_for(&joined).await;
    assert_ne!(second_token, first_token);
    let again = extract_structured(
        &McpClient::with_bearer(&server.base_url, &second_token)
            .tools_call("host.whoami", json!({}))
            .await
            .expect("whoami"),
    );
    assert_eq!(again["namespace"], json!(ns), "the grant is bound to the same tenant");
    let rows_after = server.state.db.list_live_clients(tenant.id).await.expect("clients").len();
    assert_eq!(rows_after, rows_before + 1, "a new tenant_clients row");
    assert_eq!(fake.send_count(), 0, "the join path sends no email");

    // A forged cookie gets no join option and cannot join.
    let forged = "mcphost_consent=0000";
    assert!(!flow.page(Some(forged)).await.contains("Join "));
    let refused = flow.consent(("join_workspace", "1"), Some(forged)).await;
    assert!(!refused.status().is_redirection(), "a forged cookie must not mint a code");
}
