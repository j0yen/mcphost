//! PRD-mcphost-connect-start-and-callback: the in-repo test OAuth server
//! behind `testprov`. A real axum server on an ephemeral port that speaks the
//! authorization-code + PKCE flow `host.connect.start` drives: `/authorize`
//! validates the request and redirects back with a code (or an error),
//! `/token` checks the PKCE verifier and the client credentials before it
//! hands out tokens derived from the code, so a test sees exactly which code a
//! stored token came from.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use serde_json::json;
use sha2::{Digest, Sha256};

pub const CLIENT_ID: &str = "testprov-client-id";
pub const CLIENT_SECRET: &str = "testprov-client-secret";

#[derive(Default)]
struct Inner {
    next_code: u32,
    /// code -> (code_challenge, scope)
    codes: HashMap<String, (String, String)>,
    deny: bool,
    authorize_params: Vec<HashMap<String, String>>,
    token_calls: u32,
    revoked_tokens: Vec<String>,
}

#[derive(Clone)]
pub struct TestOAuth {
    pub base: String,
    inner: Arc<Mutex<Inner>>,
}

fn pkce(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

async fn authorize(State(s): State<TestOAuth>, Query(q): Query<HashMap<String, String>>) -> Response {
    let mut inner = s.inner.lock().unwrap();
    inner.authorize_params.push(q.clone());
    let (Some(redirect), Some(state)) = (q.get("redirect_uri"), q.get("state")) else {
        return (StatusCode::BAD_REQUEST, "missing redirect_uri/state").into_response();
    };
    let location = if inner.deny {
        format!("{redirect}?error=access_denied&state={state}")
    } else {
        if q.get("client_id").map(String::as_str) != Some(CLIENT_ID)
            || q.get("code_challenge_method").map(String::as_str) != Some("S256")
            || q.get("response_type").map(String::as_str) != Some("code")
        {
            return (StatusCode::BAD_REQUEST, "bad authorize request").into_response();
        }
        inner.next_code += 1;
        let code = format!("code{}", inner.next_code);
        let challenge = q.get("code_challenge").cloned().unwrap_or_default();
        let scope = q.get("scope").cloned().unwrap_or_default();
        inner.codes.insert(code.clone(), (challenge, scope));
        format!("{redirect}?code={code}&state={state}")
    };
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

async fn token(State(s): State<TestOAuth>, Form(f): Form<HashMap<String, String>>) -> Response {
    let mut inner = s.inner.lock().unwrap();
    inner.token_calls += 1;
    let bad = |why: &str| (StatusCode::BAD_REQUEST, axum::Json(json!({"error": why}))).into_response();
    if f.get("client_id").map(String::as_str) != Some(CLIENT_ID)
        || f.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET)
    {
        return bad("invalid_client");
    }
    let Some((challenge, scope)) = f.get("code").and_then(|c| inner.codes.get(c)).cloned() else {
        return bad("invalid_grant");
    };
    if f.get("code_verifier").map(|v| pkce(v)) != Some(challenge) {
        return bad("invalid_grant");
    }
    let code = f["code"].clone();
    inner.codes.remove(&code);
    axum::Json(json!({
        "access_token": format!("access-{code}"),
        "refresh_token": format!("refresh-{code}"),
        "expires_in": 3600,
        "scope": scope,
    }))
    .into_response()
}

async fn revoke(State(s): State<TestOAuth>, Form(f): Form<HashMap<String, String>>) -> Response {
    if f.get("client_id").map(String::as_str) != Some(CLIENT_ID)
        || f.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET)
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    s.inner.lock().unwrap().revoked_tokens.push(f.get("token").cloned().unwrap_or_default());
    StatusCode::OK.into_response()
}

impl TestOAuth {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind oauth server");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let this = Self { base, inner: Arc::default() };
        let app = Router::new()
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/revoke", post(revoke))
            .with_state(this.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        this
    }

    /// The `/authorize` and `/token` URLs for `ConnectProviders::with_test_endpoints`.
    pub fn endpoints(&self) -> (String, String) {
        (format!("{}/authorize", self.base), format!("{}/token", self.base))
    }

    /// `AppState.connect_providers` with `testprov` configured against this
    /// server (and every other provider unconfigured).
    pub fn connect_providers(&self) -> mcphost::connect::ConnectProviders {
        let (auth, token) = self.endpoints();
        mcphost::connect::ConnectProviders::from_lookup(mcphost::connect::all_specs(), |k| match k {
            "MCPHOST_CONNECT_TESTPROV_CLIENT_ID" => Some(CLIENT_ID.to_string()),
            "MCPHOST_CONNECT_TESTPROV_CLIENT_SECRET" => Some(CLIENT_SECRET.to_string()),
            _ => None,
        })
        .with_test_endpoints("testprov", &auth, &token)
        .with_test_revoke_url("testprov", &format!("{}/revoke", self.base))
    }

    /// Make every later `/authorize` answer `?error=access_denied`.
    pub fn deny_consent(&self) {
        self.inner.lock().unwrap().deny = true;
    }

    /// Every query the server's `/authorize` has received, oldest first.
    pub fn authorize_requests(&self) -> Vec<HashMap<String, String>> {
        self.inner.lock().unwrap().authorize_params.clone()
    }

    /// The tokens the provider was asked to revoke, oldest first.
    pub fn revoked_tokens(&self) -> Vec<String> {
        self.inner.lock().unwrap().revoked_tokens.clone()
    }

    pub fn token_calls(&self) -> u32 {
        self.inner.lock().unwrap().token_calls
    }

    /// A browser that follows a `host.connect.start` link up to the callback
    /// but does not open it: `/connections/go/<token>` -> this server's
    /// `/authorize` -> the callback URL it redirects to.
    pub async fn follow_to_callback(&self, start_url: &str) -> String {
        let no_redirect = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client");
        let go = no_redirect.get(start_url).send().await.expect("GET go link");
        assert_eq!(go.status(), reqwest::StatusCode::FOUND, "go link must redirect to the provider");
        let authorize_url = go.headers()[header::LOCATION].to_str().unwrap().to_string();
        assert!(authorize_url.starts_with(&self.base), "go must redirect to the test OAuth server: {authorize_url}");
        let auth = no_redirect.get(&authorize_url).send().await.expect("GET authorize");
        assert_eq!(auth.status(), reqwest::StatusCode::FOUND, "authorize must redirect back");
        auth.headers()[header::LOCATION].to_str().unwrap().to_string()
    }
}
