//! A minimal, stateful OAuth authorization server -- test support only,
//! never the host (AC2/AC4's own wording: "implemented in test support,
//! not the host"). Advertises RFC 8414 metadata with a configurable
//! CIMD/DCR posture (AC2), and, when a caller drives the full flow, issues
//! a code on `/authorize`, releases it through a two-phase `consent`
//! handshake (Non-goals: no browser automation, so consent is a plain
//! JSON POST, not an HTML form), and enforces PKCE/state/client-binding/
//! replay at `/token` -- the exact surface `oauthclient`'s attack probes
//! exercise (AC4).
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, serve};
use base64::Engine as _;
use rand::RngCore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy)]
pub struct FakeAsOptions {
    pub cimd_supported: bool,
    pub none_auth_method: bool,
    pub registration_endpoint: bool,
}

impl Default for FakeAsOptions {
    fn default() -> Self {
        Self { cimd_supported: true, none_auth_method: true, registration_endpoint: false }
    }
}

struct CodeRecord {
    client_id: String,
    code_challenge: String,
    redirect_uri: String,
    state: String,
    used: bool,
}

#[derive(Clone)]
struct FakeAsState {
    opts: FakeAsOptions,
    issuer: String,
    codes: Arc<Mutex<HashMap<String, CodeRecord>>>,
}

pub struct FakeAuthServer {
    pub base_url: String,
}

pub async fn start(opts: FakeAsOptions) -> FakeAuthServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let base_url = format!("http://{addr}");
    let state = FakeAsState { opts, issuer: base_url.clone(), codes: Arc::new(Mutex::new(HashMap::new())) };

    let router = Router::new()
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .route("/authorize", get(authorize))
        .route("/consent", post(consent))
        .route("/token", post(token))
        .with_state(state);

    tokio::spawn(async move {
        let _ = serve(listener, router.into_make_service()).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    FakeAuthServer { base_url }
}

async fn metadata(State(state): State<FakeAsState>) -> Json<Value> {
    let mut body = json!({
        "issuer": state.issuer,
        "authorization_endpoint": format!("{}/authorize", state.issuer),
        "token_endpoint": format!("{}/token", state.issuer),
        "client_id_metadata_document_supported": state.opts.cimd_supported,
        "token_endpoint_auth_methods_supported":
            if state.opts.none_auth_method { vec!["none"] } else { Vec::<&str>::new() },
    });
    if state.opts.registration_endpoint {
        body["registration_endpoint"] = json!(format!("{}/register", state.issuer));
    }
    Json(body)
}

async fn authorize(State(state): State<FakeAsState>, Query(params): Query<HashMap<String, String>>) -> Json<Value> {
    let code = format!("code-{}", random_id());
    let record = CodeRecord {
        client_id: params.get("client_id").cloned().unwrap_or_default(),
        code_challenge: params.get("code_challenge").cloned().unwrap_or_default(),
        redirect_uri: params.get("redirect_uri").cloned().unwrap_or_default(),
        state: params.get("state").cloned().unwrap_or_default(),
        used: false,
    };
    state.codes.lock().unwrap().insert(code.clone(), record);
    Json(json!({"claim_code": code, "consent_endpoint": format!("{}/consent", state.issuer)}))
}

async fn consent(State(state): State<FakeAsState>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let claim_code = body.get("claim_code").and_then(Value::as_str).unwrap_or_default();
    let codes = state.codes.lock().unwrap();
    let Some(record) = codes.get(claim_code) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_claim_code"})));
    };
    (StatusCode::OK, Json(json!({"code": claim_code, "state": record.state, "iss": state.issuer})))
}

/// RFC 6749 §5.2: an error response is HTTP 400, not 200 -- the whole
/// point of `oauthclient`'s attack probes is telling "the AS refused"
/// (non-2xx, an `error` field, no token) apart from "the AS complied", so
/// every rejection branch below must carry a real error status, not a 200
/// with an `error` field a caller could easily forget to check.
fn token_error(description: &'static str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_grant", "error_description": description})))
}

async fn token(State(state): State<FakeAsState>, Form(form): Form<HashMap<String, String>>) -> (StatusCode, Json<Value>) {
    let code = form.get("code").cloned().unwrap_or_default();
    let mut codes = state.codes.lock().unwrap();
    let Some(record) = codes.get_mut(&code) else {
        return token_error("unknown code");
    };
    if record.used {
        return token_error("code already used");
    }
    if form.get("client_id").map(String::as_str) != Some(record.client_id.as_str()) {
        return token_error("client mismatch");
    }
    if form.get("redirect_uri").map(String::as_str) != Some(record.redirect_uri.as_str()) {
        return token_error("redirect_uri mismatch");
    }
    if let Some(sent_state) = form.get("state")
        && sent_state != &record.state
    {
        return token_error("state mismatch");
    }
    let Some(verifier) = form.get("code_verifier") else {
        return token_error("missing code_verifier");
    };
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if challenge != record.code_challenge {
        return token_error("pkce verification failed");
    }
    record.used = true;
    (
        StatusCode::OK,
        Json(json!({
            "access_token": format!("fake-access-{}", random_id()),
            "refresh_token": format!("fake-refresh-{}", random_id()),
            "token_type": "Bearer",
            "expires_in": 3600,
        })),
    )
}

fn random_id() -> String {
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}
