//! PRD-mcphost-federated-end-user-login: a tenant's own OIDC identity
//! provider (`host.oauth.provider_set`/`provider`/`provider_remove`,
//! `host.oauth.doctor`) and the upstream authorization-code round trip
//! `/oauth/authorize` starts when a tenant's per-tenant resource has one
//! (`GET`/`POST /oauth/federation/callback`). Reuses `authz.rs`'s client
//! registry, code/grant tables and consent-code minting -- this module
//! owns only the upstream OIDC leg and the `oauth_providers`/
//! `oauth_federation_pending` business logic.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::auth::{generate_key, hash_key};
use crate::authz::{AuthorizeParams, ClientIdentity};
use crate::claim::{html_escape, html_response, page};
use crate::db::{NewFederationPending, OauthProviderRow, Tenant};
use crate::errors::AppError;
use crate::state::AppState;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

// ---- OIDC discovery (requirement 1, AC1) -----------------------------------

const DISCOVERY_MAX_BYTES: usize = 64 * 1024;
const DISCOVERY_FETCH_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, Deserialize)]
struct DiscoveryDoc {
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    /// Not a real OIDC discovery field -- some providers extend their
    /// document with it anyway; `host.oauth.doctor`'s "callback URL listed
    /// ... when exposed" check (requirement 7) reads it when present and
    /// skips the check otherwise.
    #[serde(default)]
    redirect_uris: Option<Vec<String>>,
}

/// requirement 1: `https` only in production; test builds also accept a
/// plain `http` URL, the same `test-support`-gated relaxation
/// `authz::is_cimd_client_id` uses (a test harness's fake provider is
/// necessarily a loopback server with no CA-signed cert).
fn is_https_issuer(issuer: &str) -> bool {
    issuer.starts_with("https://") || (cfg!(feature = "test-support") && issuer.starts_with("http://"))
}

/// A dedicated client, never `AppState::http_client` -- same rationale as
/// `authz::cimd_http_client`: discovery fetch must never follow a redirect.
fn discovery_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build discovery http client")
    })
}

/// requirement 1: "no private-network targets" -- same SSRF guard
/// `authz::cimd_host_allowed` uses.
async fn discovery_host_allowed(url: &str) -> bool {
    let allow_loopback = cfg!(feature = "test-support");
    let Some((_, host)) = crate::kinds::http::split_scheme_host(url) else {
        return false;
    };
    if crate::kinds::http::is_disallowed_literal_host(host, "", allow_loopback) {
        return false;
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let Ok(lookup) = crate::kinds::http::HickoryLookup::new() else {
        return false;
    };
    match crate::kinds::http::NameLookup::lookup(&lookup, host.to_string()).await {
        Ok(ips) if ips.is_empty() => false,
        Ok(ips) => !ips.into_iter().any(|ip| crate::kinds::http::is_disallowed_ip(ip, allow_loopback)),
        Err(_) => false,
    }
}

/// requirement 1 (AC1): fetches `<issuer>/.well-known/openid-configuration`
/// once -- 5s timeout, 64KiB cap, https only, no private-network targets.
async fn fetch_discovery(issuer: &str) -> Result<DiscoveryDoc, &'static str> {
    if !is_https_issuer(issuer) {
        return Err("issuer must be an https URL");
    }
    let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
    if !discovery_host_allowed(&url).await {
        return Err("issuer host is not reachable or is a private-network target");
    }
    let resp = discovery_http_client()
        .get(&url)
        .timeout(Duration::from_secs(DISCOVERY_FETCH_TIMEOUT_SECS))
        .send()
        .await
        .map_err(|_| "discovery endpoint is unreachable")?;
    if !resp.status().is_success() {
        return Err("discovery endpoint returned an error status");
    }
    if resp.content_length().is_some_and(|n| n > DISCOVERY_MAX_BYTES as u64) {
        return Err("discovery document is too large");
    }
    let bytes = resp.bytes().await.map_err(|_| "discovery endpoint is unreachable")?;
    if bytes.len() > DISCOVERY_MAX_BYTES {
        return Err("discovery document is too large");
    }
    serde_json::from_slice::<DiscoveryDoc>(&bytes).map_err(|_| "discovery document is not valid JSON")
}

/// requirement 7 (AC8): whether `jwks_uri` answers with a JWKS-shaped
/// document -- `host.oauth.doctor`'s own check, independent of any
/// particular `kid` (unlike [`fetch_provider_jwk`], which needs a JWT's
/// `kid` to resolve one specific key).
async fn jwks_endpoint_ok(state: &AppState, jwks_uri: &str) -> bool {
    let Ok(resp) = state.http_client.get(jwks_uri).timeout(Duration::from_secs(5)).send().await else {
        return false;
    };
    if !resp.status().is_success() {
        return false;
    }
    let Ok(body) = resp.json::<Value>().await else { return false };
    body.get("keys").and_then(Value::as_array).is_some()
}

// ---- host.oauth.provider_set / provider / provider_remove ------------------

fn claims_map_value(provider: &OauthProviderRow) -> Value {
    provider
        .claims_map_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or_else(|| json!({}))
}

fn provider_row_json(row: &OauthProviderRow) -> Value {
    json!({
        "issuer": row.issuer,
        "client_id": row.client_id,
        "scopes": row.scopes,
        "claims_map": claims_map_value(row),
        "authorization_endpoint": row.authorization_endpoint,
        "token_endpoint": row.token_endpoint,
        "jwks_uri": row.jwks_uri,
        "require_verified_email": row.require_verified_email,
        "owner_login": row.owner_login,
        "created_at": row.created_unix,
    })
}

/// `host.oauth.provider_set` (requirement 1, AC1): fetches the issuer's
/// discovery document exactly once and stores it alongside the encrypted
/// client secret; re-setting (secret rotation included) replaces the row
/// entirely -- requirement 5's "keeps existing grants" holds because this
/// never touches `oauth_grants`.
pub async fn provider_set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let issuer = arg_str(args, "issuer")?;
    let client_id = arg_str(args, "client_id")?;
    let client_secret = arg_str(args, "client_secret")?;
    let scopes = match args.get("scopes") {
        None | Some(Value::Null) => "openid email profile".to_string(),
        Some(Value::Array(arr)) => {
            let mut parts = Vec::with_capacity(arr.len());
            for v in arr {
                let s = v
                    .as_str()
                    .ok_or_else(|| AppError::InvalidArgs("scopes: every element must be a string".to_string()))?;
                parts.push(s.to_string());
            }
            parts.join(" ")
        }
        Some(_) => return Err(AppError::InvalidArgs("scopes must be an array of strings".to_string())),
    };
    let claims_map_json = match args.get("claims_map") {
        None | Some(Value::Null) => None,
        Some(v @ Value::Object(_)) => Some(v.to_string()),
        Some(_) => return Err(AppError::InvalidArgs("claims_map must be an object".to_string())),
    };
    let require_verified_email = args.get("require_verified_email").and_then(Value::as_bool).unwrap_or(true);
    let owner_login = args.get("owner_login").and_then(Value::as_bool).unwrap_or(false);

    let discovery = fetch_discovery(&issuer).await.map_err(|reason| AppError::Structured {
        code: "provider_discovery_failed",
        message: format!("could not fetch discovery document for issuer '{issuer}': {reason}"),
        data: json!({"issuer": issuer, "reason": reason}),
    })?;

    let (client_secret_enc, client_secret_nonce) = state.secrets.encrypt(&client_secret)?;
    state
        .db
        .upsert_oauth_provider(
            tenant.id,
            issuer,
            client_id,
            client_secret_enc,
            client_secret_nonce,
            scopes,
            claims_map_json,
            discovery.authorization_endpoint,
            discovery.token_endpoint,
            discovery.jwks_uri,
            require_verified_email,
            owner_login,
        )
        .await?;
    provider(state, tenant).await
}

/// `host.oauth.provider` (requirement 1, AC1): `null` when this tenant has
/// none set; never carries the client secret.
pub async fn provider(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    match state.db.find_oauth_provider_by_tenant(tenant.id).await? {
        Some(row) => Ok(provider_row_json(&row)),
        None => Ok(Value::Null),
    }
}

/// `host.oauth.provider_remove` (requirement 1, AC6): revokes every
/// federated grant this provider ever produced, then removes the row --
/// the per-tenant resource falls back to owner login simply because
/// `get_authorize` no longer finds a provider row for this tenant.
pub async fn provider_remove(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let now = crate::state::now_unix();
    for grant_id in state.db.list_live_federated_oauth_grant_ids_for_tenant(tenant.id).await? {
        let _ = state.db.revoke_oauth_grant(grant_id, now).await;
        let _ = state.db.deny_oauth_jtis_for_grant(grant_id, now).await;
        let _ = state.db.revoke_oauth_refresh_tokens_for_grant(grant_id, now).await;
    }
    let removed = state.db.remove_oauth_provider(tenant.id).await?;
    if !removed {
        return Err(AppError::Structured {
            code: "provider_not_found",
            message: "no OIDC provider is registered for this tenant".to_string(),
            data: json!({}),
        });
    }
    Ok(json!({"removed": true}))
}

/// `host.oauth.doctor` (requirement 7, AC8): one line per check; a failed
/// discovery fetch never stops the remaining checks from reporting.
pub async fn doctor(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let Some(provider) = state.db.find_oauth_provider_by_tenant(tenant.id).await? else {
        return Err(AppError::Structured {
            code: "provider_not_found",
            message: "no OIDC provider is registered for this tenant".to_string(),
            data: json!({}),
        });
    };

    let discovery = fetch_discovery(&provider.issuer).await;
    let mut checks = vec![match &discovery {
        Ok(_) => json!({"name": "discovery", "status": "ok", "fix": Value::Null}),
        Err(reason) => json!({
            "name": "discovery",
            "status": "fail",
            "fix": format!(
                "the issuer's discovery document could not be fetched ({reason}); check the issuer \
                 URL and that mcphost can reach it over https."
            ),
        }),
    }];

    let jwks_ok = jwks_endpoint_ok(state, &provider.jwks_uri).await;
    checks.push(json!({
        "name": "jwks",
        "status": if jwks_ok { "ok" } else { "fail" },
        "fix": if jwks_ok {
            Value::Null
        } else {
            json!("the provider's jwks_uri could not be fetched or is not a JWKS document; check \
                   network egress and the stored jwks_uri.")
        },
    }));

    let resource = crate::authz::per_tenant_resource(state, &tenant.namespace);
    checks.push(json!({
        "name": "metadata",
        "status": "ok",
        "fix": Value::Null,
        "detail": format!("this tenant's resource is {resource}"),
    }));

    let callback = federation_callback_url(state);
    checks.push(match &discovery {
        Ok(doc) => match &doc.redirect_uris {
            Some(uris) if !uris.iter().any(|u| u == &callback) => json!({
                "name": "callback_registered",
                "status": "fail",
                "fix": format!(
                    "the provider's discovery document does not list {callback} in redirect_uris; \
                     add it in the provider's dashboard."
                ),
            }),
            _ => json!({"name": "callback_registered", "status": "ok", "fix": Value::Null}),
        },
        // requirement 7: "when exposed" -- most providers don't publish
        // this at all, and discovery being unreachable can't be blamed on
        // the callback registration specifically, so this reports `ok`
        // (nothing to check) rather than compounding the discovery failure.
        Err(_) => json!({"name": "callback_registered", "status": "ok", "fix": Value::Null}),
    });

    let dry_authorize_url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state=<state>&nonce=<nonce>&\
         code_challenge=<challenge>&code_challenge_method=S256",
        provider.authorization_endpoint,
        percent_encode(&provider.client_id),
        percent_encode(&callback),
        percent_encode(&provider.scopes),
    );
    checks.push(json!({
        "name": "dry_authorize",
        "status": "ok",
        "fix": Value::Null,
        "detail": dry_authorize_url,
    }));

    Ok(json!({"checks": checks}))
}

// ---- upstream round trip (requirement 2, AC2/AC4/AC5) ----------------------

fn federation_callback_url(state: &AppState) -> String {
    format!("{}/oauth/federation/callback", state.public_url.trim_end_matches('/'))
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// RFC 7636 S256.
fn pkce_challenge_s256(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// requirement 3: a federated end user's subject, namespaced by the
/// provider's own issuer so two providers can never collide on the same
/// raw `sub` (Technical considerations).
fn namespaced_subject(issuer: &str, sub: &str) -> String {
    format!("{issuer}#{sub}")
}

fn render_inline_error(code: &str, message: &str) -> Response {
    html_response(
        StatusCode::BAD_REQUEST,
        page(
            "mcphost — authorize",
            &format!("<h1>{}</h1><p>{}</p>", html_escape(code), html_escape(message)),
        ),
    )
}

fn render_federation_consent_page(client_name: &str, resource: &str, token: &str) -> String {
    page(
        "mcphost — authorize",
        &format!(
            "<h1>Authorize {client}</h1>\
             <p>{client} is requesting access to <code>{resource}</code> on your behalf.</p>\
             <form method=\"post\" action=\"/oauth/federation/callback\">\
             <input type=\"hidden\" name=\"token\" value=\"{token}\">\
             <button type=\"submit\">Approve</button>\
             </form>",
            client = html_escape(client_name),
            resource = html_escape(resource),
            token = html_escape(token),
        ),
    )
}

/// requirement 2/AC2: starts the upstream leg of a federated login --
/// persists the pending row, then redirects the browser to the provider's
/// own authorization endpoint with PKCE S256, a fresh `nonce`, and an
/// opaque `state` bound to that row.
#[allow(clippy::too_many_arguments)]
pub async fn start(
    state: &AppState,
    tenant: &Tenant,
    provider: &OauthProviderRow,
    identity: &ClientIdentity,
    redirect_uri: &str,
    resource: &str,
    params: &AuthorizeParams,
) -> Response {
    let pkce_verifier = generate_key();
    let challenge = pkce_challenge_s256(&pkce_verifier);
    let nonce = generate_key();
    let upstream_state = generate_key();
    let now = crate::state::now_unix();

    let new = NewFederationPending {
        upstream_state: upstream_state.clone(),
        tenant_id: tenant.id,
        client_id: identity.client_id.clone(),
        client_name: identity.client_name.clone(),
        method: identity.method.to_string(),
        redirect_uri: redirect_uri.to_string(),
        code_challenge: params.code_challenge.clone().unwrap_or_default(),
        resource: resource.to_string(),
        scope: params.scope.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "mcp".to_string()),
        original_state: params.state.clone().unwrap_or_default(),
        nonce: nonce.clone(),
        pkce_verifier: pkce_verifier.clone(),
    };
    match state.db.insert_oauth_federation_pending(new, now).await {
        Ok(true) => {}
        Ok(false) => {
            return render_inline_error(
                "too_many_pending_logins",
                "too many pending federated logins for this tenant right now; try again shortly",
            );
        }
        Err(_) => return render_inline_error("server_error", "could not start federated login"),
    }

    let callback = federation_callback_url(state);
    let Ok(mut url) = reqwest::Url::parse(&provider.authorization_endpoint) else {
        return render_inline_error("server_error", "this tenant's provider has an invalid authorization endpoint");
    };
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("client_id", &provider.client_id);
        pairs.append_pair("redirect_uri", &callback);
        pairs.append_pair("response_type", "code");
        pairs.append_pair("scope", &provider.scopes);
        pairs.append_pair("state", &upstream_state);
        pairs.append_pair("nonce", &nonce);
        pairs.append_pair("code_challenge", &challenge);
        pairs.append_pair("code_challenge_method", "S256");
    }
    Redirect::to(url.as_str()).into_response()
}

// ---- the provider's own JWKS (id_token signature verification) -----------

#[derive(Debug, Deserialize)]
struct ProviderJwk {
    kty: String,
    kid: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    crv: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProviderJwkSet {
    keys: Vec<ProviderJwk>,
}

/// requirement 2: resolves the specific key an id_token's `kid` names (or
/// the sole key, for a single-key JWKS) -- a plain per-call fetch rather
/// than `oauth::JwksCache`'s cross-request cache, since id_token
/// verification happens once per login, not once per API call.
async fn fetch_provider_jwk(state: &AppState, jwks_uri: &str, kid: Option<&str>) -> Option<(DecodingKey, Algorithm)> {
    let resp = state.http_client.get(jwks_uri).timeout(Duration::from_secs(5)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let doc: ProviderJwkSet = resp.json().await.ok()?;
    let jwk = match kid {
        Some(k) => doc.keys.iter().find(|j| j.kid.as_deref() == Some(k))?,
        None if doc.keys.len() == 1 => &doc.keys[0],
        None => return None,
    };
    match jwk.kty.as_str() {
        "RSA" => Some((DecodingKey::from_rsa_components(jwk.n.as_deref()?, jwk.e.as_deref()?).ok()?, Algorithm::RS256)),
        "EC" if jwk.crv.as_deref() == Some("P-256") => {
            Some((DecodingKey::from_ec_components(jwk.x.as_deref()?, jwk.y.as_deref()?).ok()?, Algorithm::ES256))
        }
        _ => None,
    }
}

fn mapped_claim(claims: &Value, map: &Value, our_name: &str) -> Option<String> {
    let provider_name = map.get(our_name).and_then(Value::as_str).unwrap_or(our_name);
    claims.get(provider_name).and_then(Value::as_str).map(str::to_string)
}

#[derive(Debug, Deserialize)]
pub struct FederationCallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// `GET /oauth/federation/callback` (requirement 2, AC2/AC4/AC5): the
/// provider redirects here with `code`/`state` (or `error`/`state`).
/// Exchanges the code, validates the `id_token`, then renders mcphost's
/// own consent page -- every failure renders inline with a reason code and
/// mints no code for the original client (AC4).
pub async fn get_callback(State(state): State<Arc<AppState>>, Query(q): Query<FederationCallbackQuery>) -> Response {
    let Some(upstream_state) = q.state.filter(|s| !s.is_empty()) else {
        return render_inline_error("unknown_state", "this login link is missing its state parameter");
    };
    let Ok(Some(row)) = state.db.find_oauth_federation_pending_by_state(upstream_state.clone()).await else {
        return render_inline_error("unknown_state", "this login link is unknown or was already used");
    };
    let now = crate::state::now_unix();
    if now > row.expires_unix {
        return render_inline_error("state_expired", "this login link has expired; ask your client to try again");
    }
    if row.status != "pending" {
        return render_inline_error("unknown_state", "this login link was already used");
    }
    if let Some(err) = q.error.filter(|s| !s.is_empty()) {
        return render_inline_error(&err, "the identity provider refused this login");
    }
    let Some(code) = q.code.filter(|s| !s.is_empty()) else {
        return render_inline_error("invalid_request", "the identity provider did not return a code");
    };
    let Ok(Some(provider)) = state.db.find_oauth_provider_by_tenant(row.tenant_id).await else {
        return render_inline_error("provider_removed", "this tenant no longer has an identity provider registered");
    };
    let Ok(client_secret) = state.secrets.decrypt(&provider.client_secret_enc, &provider.client_secret_nonce) else {
        return render_inline_error("server_error", "could not decrypt this tenant's provider credentials");
    };

    let send_result = state
        .http_client
        .post(&provider.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", federation_callback_url(&state).as_str()),
            ("client_id", provider.client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code_verifier", row.pkce_verifier.as_str()),
        ])
        .send()
        .await;
    let response = match send_result {
        Ok(r) if r.status().is_success() => r,
        _ => return render_inline_error("token_exchange_failed", "could not exchange the code with the identity provider"),
    };
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let Some(id_token) = body.get("id_token").and_then(Value::as_str).map(str::to_string) else {
        return render_inline_error("invalid_id_token", "the identity provider did not return an id_token");
    };

    let header = match jsonwebtoken::decode_header(&id_token) {
        Ok(h) => h,
        Err(_) => return render_inline_error("invalid_id_token", "the id_token is malformed"),
    };
    let Some((decoding_key, alg)) = fetch_provider_jwk(&state, &provider.jwks_uri, header.kid.as_deref()).await else {
        return render_inline_error("invalid_id_token", "could not resolve a key to verify the id_token's signature");
    };
    let mut validation = Validation::new(alg);
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    let claims = match jsonwebtoken::decode::<Value>(&id_token, &decoding_key, &validation) {
        Ok(data) => data.claims,
        Err(_) => return render_inline_error("invalid_id_token", "the id_token's signature does not verify"),
    };

    if claims.get("iss").and_then(Value::as_str) != Some(provider.issuer.as_str()) {
        return render_inline_error("invalid_id_token", "the id_token's iss does not match this provider");
    }
    let aud_ok = match claims.get("aud") {
        Some(Value::String(s)) => s == &provider.client_id,
        Some(Value::Array(arr)) => arr.iter().any(|v| v.as_str() == Some(provider.client_id.as_str())),
        _ => false,
    };
    if !aud_ok {
        return render_inline_error("invalid_id_token", "the id_token's aud does not match this client_id");
    }
    let exp_ok = claims.get("exp").and_then(Value::as_i64).is_some_and(|exp| now <= exp + 60);
    if !exp_ok {
        return render_inline_error("invalid_id_token", "the id_token has expired");
    }
    if claims.get("nonce").and_then(Value::as_str) != Some(row.nonce.as_str()) {
        return render_inline_error("nonce_mismatch", "the id_token's nonce does not match this login attempt");
    }
    let Some(sub) = claims.get("sub").and_then(Value::as_str).map(str::to_string) else {
        return render_inline_error("invalid_id_token", "the id_token has no sub claim");
    };

    let claims_map = claims_map_value(&provider);
    let email = mapped_claim(&claims, &claims_map, "email");
    let name = mapped_claim(&claims, &claims_map, "name");
    let email_verified = claims.get("email_verified").and_then(Value::as_bool);
    if provider.require_verified_email && email_verified == Some(false) {
        return render_inline_error("email_unverified", "this provider reports the end user's email as unverified");
    }

    let namespaced = namespaced_subject(&provider.issuer, &sub);
    match state
        .db
        .mark_oauth_federation_verified(upstream_state.clone(), namespaced, email, name, provider.issuer.clone())
        .await
    {
        Ok(true) => {}
        _ => return render_inline_error("unknown_state", "this login link was already used"),
    }

    let client_name = row.client_name.clone().unwrap_or_else(|| row.client_id.clone());
    html_response(StatusCode::OK, render_federation_consent_page(&client_name, &row.resource, &upstream_state))
}

#[derive(Deserialize)]
pub struct FederationConsentForm {
    #[serde(default)]
    token: Option<String>,
}

// HLT-010: the consent token is a bearer secret; never let {:?} print it.
impl std::fmt::Debug for FederationConsentForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FederationConsentForm")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// `POST /oauth/federation/callback` (requirement 2, AC2): the browser's
/// approval of mcphost's own consent page -- mints a code for the
/// *original* client (never the identity provider) and redirects there
/// with `code`/`state`, exactly like `authz::mint_code_and_redirect`.
pub async fn post_callback(State(state): State<Arc<AppState>>, axum::Form(form): axum::Form<FederationConsentForm>) -> Response {
    let Some(token) = form.token.filter(|s| !s.is_empty()) else {
        return render_inline_error("unknown_state", "missing token");
    };
    let Ok(Some(row)) = state.db.find_oauth_federation_pending_by_state(token.clone()).await else {
        return render_inline_error("unknown_state", "this login link is unknown or was already used");
    };
    if row.status != "verified" {
        return render_inline_error("unknown_state", "this login link was already used or is not ready yet");
    }
    let now = crate::state::now_unix();
    if now > row.expires_unix {
        return render_inline_error("state_expired", "this login link has expired");
    }
    if !state.db.claim_oauth_federation_pending(token).await.unwrap_or(false) {
        return render_inline_error("unknown_state", "this login link was already used");
    }

    let code = generate_key();
    let code_hash = hash_key(&code);
    let expires_unix = now + crate::authz::AUTHORIZATION_CODE_TTL_SECS;
    let inserted = state
        .db
        .insert_oauth_code(
            code_hash,
            row.tenant_id,
            row.client_id.clone(),
            row.client_name.clone(),
            row.method.clone(),
            row.redirect_uri.clone(),
            row.code_challenge.clone(),
            row.resource.clone(),
            row.scope.clone(),
            expires_unix,
            row.end_user(),
        )
        .await;
    if inserted.is_err() {
        return render_inline_error("server_error", "could not mint an authorization code");
    }
    let Ok(mut url) = reqwest::Url::parse(&row.redirect_uri) else {
        return render_inline_error("invalid_request", "redirect_uri is malformed");
    };
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("code", &code);
        pairs.append_pair("state", &row.original_state);
    }
    Redirect::to(url.as_str()).into_response()
}
