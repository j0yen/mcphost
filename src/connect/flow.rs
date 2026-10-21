//! PRD-mcphost-connect-start-and-callback: the tenant-level connect flow.
//! `host.connect.start` mints a single-use `/connections/go/<token>` link,
//! `/connections/callback/<provider>` exchanges the code with the host-owned
//! client credentials and stores the tokens under the sentinel subject
//! `tenant`, and `host.connect.list` reads them back. It reuses the vault's
//! `vault_handoff_tokens` / `vault_tokens` tables, PKCE and page helpers; the
//! end-user path in `vault.rs` is untouched.
//!
//! The handoff row's `oauth_state` column carries `<random>:<scopes>` until
//! the callback consumes the row (then `used:` is prefixed), so the
//! requested scopes and the "callback already ran" mark need no new column.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Provider;
use crate::auth::{generate_key, hash_key};
use crate::db::{Tenant, VaultHandoffOutcome};
use crate::errors::AppError;
use crate::state::{AppState, now_unix};
use crate::vault::{
    HANDOFF_TTL_SECS, html_response, page, percent_encode, pkce_challenge_s256, render_expired,
    render_storage_error, render_upstream_error,
};

/// The `vault_tokens.end_user_subject` of a tenant-level connection.
pub const TENANT_SUBJECT: &str = "tenant";

/// The code-exchange call's timeout.
const EXCHANGE_TIMEOUT_SECS: u64 = 10;

crate::err_code! {
    pub PROVIDER_UNAVAILABLE,
    code: "provider_unavailable", family: Oauth, class: HostFault, retryable: false,
    remedy: "The host has no OAuth client for this provider yet; the operator sets its MCPHOST_CONNECT_<PROVIDER>_CLIENT_ID and _CLIENT_SECRET (host.connect.providers shows the reason).",
}

crate::err_code! {
    pub SCOPE_UNKNOWN,
    code: "scope_unknown", family: Oauth, class: AgentFault, retryable: false,
    remedy: "Pass only scope names host.connect.providers lists for this provider, or omit scopes for the default set.",
}

fn parse_provider(args: &Value) -> Result<Provider, AppError> {
    let name = args
        .get("provider")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'provider'".to_string()))?;
    Provider::parse(name).ok_or_else(|| {
        AppError::InvalidParams(format!(
            "unknown provider '{name}': expected one of {}",
            super::provider_names_csv()
        ))
    })
}

/// `scopes`: the caller's array, else every scope the registry names.
fn requested_scopes(provider: Provider, args: &Value) -> Result<Vec<String>, AppError> {
    let scopes: Vec<String> = match args.get("scopes") {
        None | Some(Value::Null) => provider.spec().scopes.iter().map(|s| s.name.to_string()).collect(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| AppError::InvalidArgs("scopes: every element must be a string".to_string()))
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(AppError::InvalidArgs("scopes must be an array of strings".to_string())),
    };
    if let Err(unknown) = provider.validate_scopes(&scopes) {
        return Err(AppError::Structured {
            code: SCOPE_UNKNOWN,
            message: format!("unknown scope(s) for {}: {}", provider.name(), unknown.join(", ")),
            data: json!({"unknown": unknown}),
        });
    }
    Ok(scopes)
}

/// Mint a handoff row for `scopes` and return its `/connections/go/<token>` URL.
async fn mint_link(state: &AppState, tenant: &Tenant, provider: Provider, scopes: &[String]) -> Result<String, AppError> {
    let token = generate_key();
    let verifier = generate_key();
    let (verifier_enc, verifier_nonce) = state.secrets.encrypt(&verifier)?;
    let slot = format!("{}:{}", generate_key(), scopes.join(" "));
    state
        .db
        .create_vault_handoff_token(
            tenant.id,
            hash_key(&token),
            provider.name().to_string(),
            TENANT_SUBJECT.to_string(),
            None,
            verifier_enc,
            verifier_nonce,
            slot,
            now_unix() + HANDOFF_TTL_SECS,
        )
        .await?;
    Ok(format!("{}/connections/go/{}", state.public_url.trim_end_matches('/'), token))
}

/// `host.connect.start {provider, scopes?}`.
pub async fn start(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let provider = parse_provider(args)?;
    let scopes = requested_scopes(provider, args)?;
    if let Some(reason) = state.connect_providers.reason(provider.name()) {
        return Err(AppError::Structured {
            code: PROVIDER_UNAVAILABLE,
            message: format!("provider '{}' is not available on this host: {reason}", provider.name()),
            data: json!({"provider": provider.name(), "reason": reason}),
        });
    }
    let url = mint_link(state, tenant, provider, &scopes).await?;
    Ok(json!({
        "url": url,
        "expires_in_s": HANDOFF_TTL_SECS,
        "provider": provider.name(),
        "scopes": scopes,
    }))
}

/// `host.connect.list`: one row per registry provider.
pub async fn list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let mut rows = Vec::with_capacity(Provider::ALL.len());
    for &provider in Provider::ALL {
        let name = provider.name();
        let token = state.db.get_vault_token(tenant.id, name.to_string(), TENANT_SUBJECT.to_string()).await?;
        let mut row = match token {
            Some(t) => json!({
                "name": name,
                "connected": t.revoked_unix.is_none(),
                "scopes": t.scopes.split_whitespace().collect::<Vec<_>>(),
                "expires_at": if t.expires_unix == i64::MAX { Value::Null } else { json!(t.expires_unix) },
                "connected_at": t.connected_unix,
                "last_refreshed_at": t.last_refreshed_unix,
                "revoked_at": t.revoked_unix,
                "revoked_reason": t.revoked_reason,
            }),
            None => json!({
                "name": name,
                "connected": false,
                "scopes": [],
                "expires_at": Value::Null,
                "connected_at": Value::Null,
                "last_refreshed_at": Value::Null,
                "revoked_at": Value::Null,
                "revoked_reason": Value::Null,
            }),
        };
        if row["revoked_at"].is_i64()
            && state.connect_providers.reason(name).is_none()
            && let Ok(url) = {
                let scopes: Vec<String> = row["scopes"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                mint_link(state, tenant, provider, &scopes).await
            }
            && let Value::Object(m) = &mut row
        {
            m.insert("reconnect_url".to_string(), json!(url));
        }
        rows.push(row);
    }
    Ok(json!({"providers": rows}))
}

/// The `revoked_reason` `host.connect.revoke` records.
pub const REVOKE_REASON: &str = "tenant_revoke";

/// How long `host.connect.revoke` waits for the provider's revocation endpoint.
const REVOKE_TIMEOUT_SECS: u64 = 5;

/// `host.connect.revoke {provider}`: revokes locally first, then asks the
/// provider to revoke the token when its registry row has a `revoke_url`.
/// Idempotent.
pub async fn revoke(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let provider = parse_provider(args)?;
    let name = provider.name();
    let before = state.db.get_vault_token(tenant.id, name.to_string(), TENANT_SUBJECT.to_string()).await?;
    let existed = state.db.revoke_tenant_vault_token(tenant.id, name.to_string(), REVOKE_REASON.to_string()).await?;
    let row = state.db.get_vault_token(tenant.id, name.to_string(), TENANT_SUBJECT.to_string()).await?;
    // Only the call that flips a live row asks the provider; a repeat has nothing left to revoke.
    let provider_revoked = match (&before, state.connect_providers.revoke_url(provider.spec())) {
        (Some(t), Some(url)) if t.revoked_unix.is_none() => {
            call_revoke_url(state, provider, &url, t).await
        }
        _ => false,
    };
    Ok(json!({
        "provider": name,
        "revoked": existed,
        "revoked_at": row.as_ref().and_then(|r| r.revoked_unix),
        "revoked_reason": row.and_then(|r| r.revoked_reason),
        "provider_revoked": provider_revoked,
    }))
}

/// RFC 7009 token revocation, best effort: `true` only on a 2xx within the timeout.
async fn call_revoke_url(state: &AppState, provider: Provider, url: &str, t: &crate::db::VaultTokenRow) -> bool {
    let Some((client_id, client_secret)) = state.connect_providers.credentials(provider.name()) else {
        return false;
    };
    // Prefer the refresh token (revoking it kills the access token too).
    let (token, hint) = match (&t.refresh_enc, &t.refresh_nonce) {
        (Some(enc), Some(nonce)) => (state.secrets.decrypt(enc, nonce), "refresh_token"),
        _ => (state.secrets.decrypt(&t.access_enc, &t.access_nonce), "access_token"),
    };
    let Ok(token) = token else { return false };
    state
        .http_client
        .post(url)
        .timeout(std::time::Duration::from_secs(REVOKE_TIMEOUT_SECS))
        .form(&[
            ("token", token.as_str()),
            ("token_type_hint", hint),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ])
        .send()
        .await
        .is_ok_and(|r| r.status().is_success())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn render_state_invalid() -> String {
    page(
        "mcphost — link not valid",
        "<h1>state_invalid</h1><p>This authorization link is unknown, expired or was already used. \
         Ask your agent for a new one.</p>",
    )
}

/// `GET /connections/go/{token}`: the single-use gate, then a redirect to the
/// provider's authorize URL.
pub async fn get_go(State(state): State<Arc<AppState>>, Path(token): Path<String>) -> Response {
    let token_hash = hash_key(&token);
    // A vault (end-user) handoff token is not redeemable here, and must not be burnt by trying.
    match state.db.find_vault_handoff_by_token_hash(token_hash.clone()).await {
        Ok(Some(row)) if row.end_user_subject != TENANT_SUBJECT => {
            return html_response(StatusCode::GONE, render_expired());
        }
        Ok(_) => {}
        Err(_) => return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
    }
    let row = match state.db.redeem_vault_handoff_token(token_hash).await {
        Ok(VaultHandoffOutcome::Redeemed(row)) => row,
        Ok(VaultHandoffOutcome::NotFound | VaultHandoffOutcome::Expired | VaultHandoffOutcome::AlreadyRedeemed) => {
            return html_response(StatusCode::GONE, render_expired());
        }
        Err(_) => return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
    };
    let Some(provider) = Provider::parse(&row.provider) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let Some((client_id, _)) = state.connect_providers.credentials(provider.name()) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let Ok(verifier) = state.secrets.decrypt(&row.pkce_verifier_enc, &row.pkce_verifier_nonce) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let scopes = row.oauth_state.split_once(':').map(|(_, s)| s).unwrap_or_default();
    let spec = provider.spec();
    let (auth_url, _) = state.connect_providers.endpoints(spec);
    let redirect_uri = format!("{}/connections/callback/{}", state.public_url.trim_end_matches('/'), provider.name());
    let mut query = format!(
        "client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        percent_encode(client_id),
        percent_encode(&redirect_uri),
        percent_encode(scopes),
        percent_encode(&token),
        percent_encode(&pkce_challenge_s256(&verifier)),
    );
    for (k, v) in spec.extra_authorize_params.iter() {
        query.push_str(&format!("&{}={}", percent_encode(k), percent_encode(v)));
    }
    let sep = if auth_url.contains('?') { '&' } else { '?' };
    let Ok(value) = HeaderValue::from_str(&format!("{auth_url}{sep}{query}")) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    match Response::builder().status(StatusCode::FOUND).header(header::LOCATION, value).body(Body::empty()) {
        Ok(resp) => resp,
        Err(_) => html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
    }
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// `GET /connections/callback/{provider}`.
pub async fn get_callback(
    State(state): State<Arc<AppState>>,
    Path(provider_name): Path<String>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let invalid = || html_response(StatusCode::BAD_REQUEST, render_state_invalid());
    let (Some(provider), Some(raw_state)) = (Provider::parse(&provider_name), q.state) else {
        return invalid();
    };
    let token_hash = hash_key(&raw_state);
    let row = match state.db.find_vault_handoff_by_token_hash(token_hash.clone()).await {
        Ok(Some(row)) => row,
        Ok(None) => return invalid(),
        Err(_) => return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
    };
    if row.provider != provider.name()
        || row.end_user_subject != TENANT_SUBJECT
        || row.redeemed_unix.is_none()
        || row.oauth_state.starts_with("used:")
        || now_unix() > row.expires_unix
    {
        return invalid();
    }
    match state.db.consume_vault_handoff_callback(token_hash).await {
        Ok(true) => {}
        Ok(false) => return invalid(),
        Err(_) => return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
    }

    if let Some(error) = q.error {
        return html_response(
            StatusCode::BAD_REQUEST,
            page(
                "mcphost — not connected",
                &format!(
                    "<h1>Not connected</h1><p>{} did not authorize the connection: <code>{}</code>.</p>",
                    escape(provider.name()),
                    escape(&error)
                ),
            ),
        );
    }
    let Some(code) = q.code else {
        return html_response(StatusCode::BAD_REQUEST, render_upstream_error());
    };

    let Some((client_id, client_secret)) = state.connect_providers.credentials(provider.name()) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let Ok(verifier) = state.secrets.decrypt(&row.pkce_verifier_enc, &row.pkce_verifier_nonce) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let spec = provider.spec();
    let (_, token_url) = state.connect_providers.endpoints(spec);
    let redirect_uri = format!("{}/connections/callback/{}", state.public_url.trim_end_matches('/'), provider.name());
    let response = match state
        .http_client
        .post(&token_url)
        .header(header::ACCEPT, "application/json")
        .timeout(std::time::Duration::from_secs(EXCHANGE_TIMEOUT_SECS))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        _ => return html_response(StatusCode::BAD_GATEWAY, render_upstream_error()),
    };
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let Some(access_token) = body.get("access_token").and_then(Value::as_str) else {
        return html_response(StatusCode::BAD_GATEWAY, render_upstream_error());
    };
    let refresh_token = body.get("refresh_token").and_then(Value::as_str);
    let expires_unix = match (spec.refresh, body.get("expires_in").and_then(Value::as_i64)) {
        (super::Refresh::None, None) => i64::MAX,
        (_, expires_in) => now_unix() + expires_in.unwrap_or(3600),
    };
    let requested = row.oauth_state.split_once(':').map(|(_, s)| s).unwrap_or_default();
    let scopes = match body.get("scope").and_then(Value::as_str) {
        Some(granted) if !granted.trim().is_empty() => granted.replace(',', " "),
        _ => requested.to_string(),
    };

    let Ok((access_enc, access_nonce)) = state.secrets.encrypt(access_token) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let (refresh_enc, refresh_nonce) = match refresh_token.map(|rt| state.secrets.encrypt(rt)) {
        Some(Ok((e, n))) => (Some(e), Some(n)),
        Some(Err(_)) => return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
        None => (None, None),
    };
    if state
        .db
        .upsert_tenant_vault_token(
            row.tenant_id,
            provider.name().to_string(),
            access_enc,
            access_nonce,
            refresh_enc,
            refresh_nonce,
            expires_unix,
            scopes,
        )
        .await
        .is_err()
    {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    }

    let tenant_name = match state.db.find_tenant_by_id(row.tenant_id).await {
        Ok(Some(t)) => t.display_name,
        _ => String::new(),
    };
    html_response(
        StatusCode::OK,
        page(
            "mcphost — connected",
            &format!(
                "<h1>Connected</h1><p>{} connected to tenant {}; you can close this tab.</p>",
                escape(provider.name()),
                escape(&tenant_name)
            ),
        ),
    )
}
