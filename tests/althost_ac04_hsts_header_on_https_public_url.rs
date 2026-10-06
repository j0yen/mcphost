//! PRD-mcphost-reachability-alt-host
//! AC4 (P0) — Given `MCPHOST_PUBLIC_URL` is https, When any response is
//! served, Then it carries `Strict-Transport-Security` with
//! `max-age >= 15552000`.
//!
//! Production terminates TLS at Caddy and this binary always serves
//! plain HTTP behind it (same as every other route in this crate) -- the
//! HSTS gate is purely on `state.public_url`'s own configured scheme, not
//! on the connection the test itself makes, so overriding just that
//! string on an otherwise-ordinary [`common::bare_state`] and driving it
//! with a real (plain-HTTP) listener proves the gate correctly.

use crate::common;
use std::sync::Arc;

async fn start_bare_server(public_url: &str) -> (common::TempDataDir, String) {
    let dir = common::TempDataDir::new();
    let mut state = common::bare_state(&dir.0).await;
    state.public_url = public_url.to_string();
    let state = Arc::new(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let serve_state = state.clone();
    tokio::spawn(async move {
        let _ = mcphost::http::serve_on_listener(listener, serve_state).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (dir, format!("http://{addr}"))
}

#[tokio::test]
async fn hsts_present_with_sufficient_max_age_when_public_url_is_https() {
    let (_dir, base_url) = start_bare_server("https://mcphost.dev").await;
    let resp = reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .send()
        .await
        .expect("GET /healthz");
    let hsts = resp
        .headers()
        .get("strict-transport-security")
        .expect("Strict-Transport-Security header present")
        .to_str()
        .expect("ascii header value")
        .to_string();
    let max_age: u64 = hsts
        .split("max-age=")
        .nth(1)
        .and_then(|s| s.split(';').next())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| panic!("no parseable max-age in {hsts:?}"));
    assert!(max_age >= 15_552_000, "max-age {max_age} must be >= 15552000");
}

#[tokio::test]
async fn no_hsts_when_public_url_is_plain_http() {
    let (_dir, base_url) = start_bare_server("http://127.0.0.1:0").await;
    let resp = reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .send()
        .await
        .expect("GET /healthz");
    assert!(
        resp.headers().get("strict-transport-security").is_none(),
        "an http public_url must never get HSTS"
    );
}
