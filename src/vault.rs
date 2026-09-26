//! PRD-mcphost-upstream-token-vault: an end user's Slack/Google/Stripe
//! token lives in the host, never in the model. `host.vault.provider_set`/
//! `providers` own a tenant's registered upstream OAuth application
//! (requirement 2); `connect_link` mints the one-time
//! `/vault/connect/<token>` handoff a human's browser completes
//! (requirement 3, this module's own `get_connect`/`get_callback` routes,
//! wired in `http.rs`); [`resolve_for_call`] is what `handler.rs`'s real
//! dispatch path calls before `Kind::call` runs to resolve (and, inline,
//! refresh) the calling end user's stored token for an http-kind tool
//! declaring `upstream_provider` (requirement 4); `disconnect` revokes it
//! (requirement 5/6).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::auth::{generate_key, hash_key};
use crate::db::{Tenant, VaultHandoffOutcome};
use crate::enduser::EndUser;
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// requirement 3: how long a `host.vault.connect_link` token stays
/// redeemable.
const HANDOFF_TTL_SECS: i64 = 15 * 60;

/// requirement 4: a call refreshes inline once a stored token is within
/// this many seconds of `expires_unix` (AC5's "expiring in 60 s" is well
/// inside this window, so it always triggers a refresh).
const REFRESH_WINDOW_SECS: i64 = 120;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

/// `scopes`: a JSON array of strings, joined space-delimited (the shape
/// every OAuth `scope` parameter and stored-scope string this module reads
/// back expects).
fn arg_scopes(args: &Value) -> Result<String, AppError> {
    let arr = args
        .get("scopes")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'scopes' (array of strings)".to_string()))?;
    let mut parts = Vec::with_capacity(arr.len());
    for v in arr {
        let s = v
            .as_str()
            .ok_or_else(|| AppError::InvalidArgs("scopes: every element must be a string".to_string()))?;
        parts.push(s.to_string());
    }
    Ok(parts.join(" "))
}

// ---- host.vault.* tools -----------------------------------------------

/// `host.vault.provider_set`'s three OAuth presets (P1 requirement 4,
/// AC7): an explicit `auth_url`/`token_url` in the call always overrides
/// the matching preset value, field by field, not as an all-or-nothing
/// choice. An unrecognized preset is `invalid_params` -- the wire code
/// AC7 itself pins, distinct from [`AppError::InvalidArgs`]'s own
/// `args_invalid`.
fn preset_urls(preset: &str) -> Result<(&'static str, &'static str), AppError> {
    match preset {
        "slack" => Ok((
            "https://slack.com/oauth/v2/authorize",
            "https://slack.com/api/oauth.v2.access",
        )),
        "github" => Ok((
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
        )),
        "google" => Ok((
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
        )),
        other => Err(AppError::InvalidParams(format!(
            "unknown preset '{other}': expected 'slack', 'github', or 'google'"
        ))),
    }
}

/// `host.vault.provider_set` (requirement 2, AC11; P1 requirement 4, AC7): a
/// re-set of an already registered provider name never counts against
/// `vault_providers_max`, same "existing name is free" convention
/// `control::secret_set` already uses for `secrets_max`.
pub async fn provider_set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let preset = match args.get("preset") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(preset_urls(s)?),
        Some(_) => return Err(AppError::InvalidArgs("preset must be a string".to_string())),
    };
    let auth_url = match args.get("auth_url").and_then(Value::as_str) {
        Some(s) => s.to_string(),
        None => preset
            .map(|(a, _)| a.to_string())
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'auth_url' (or a 'preset')".to_string()))?,
    };
    let token_url = match args.get("token_url").and_then(Value::as_str) {
        Some(s) => s.to_string(),
        None => preset
            .map(|(_, t)| t.to_string())
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'token_url' (or a 'preset')".to_string()))?,
    };
    let client_id = arg_str(args, "client_id")?;
    let client_secret = arg_str(args, "client_secret")?;
    let scopes = arg_scopes(args)?;

    let already_exists = state.db.list_vault_provider_names(tenant.id).await?.contains(&name);
    if !already_exists {
        let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
            AppError::Internal(format!(
                "tenant's plan '{}' is not in the loaded plan catalog",
                tenant.plan
            ))
        })?;
        let count = state.db.count_vault_providers(tenant.id).await?;
        if count >= plan.vault_providers_max {
            return Err(AppError::Structured {
                code: "quota_vault_providers",
                message: format!(
                    "plan '{}' quota exceeded: vault_providers_max (limit {})",
                    tenant.plan, plan.vault_providers_max
                ),
                data: json!({
                    "quota": "vault_providers_max",
                    "limit": plan.vault_providers_max,
                    "used": count,
                }),
            });
        }
    }

    let (client_secret_enc, client_secret_nonce) = state.secrets.encrypt(&client_secret)?;
    state
        .db
        .upsert_vault_provider(
            tenant.id,
            name.clone(),
            auth_url,
            token_url,
            client_id,
            client_secret_enc,
            client_secret_nonce,
            scopes,
        )
        .await?;
    Ok(json!({"name": name, "set": true}))
}

/// `host.vault.providers` (AC9): `client_secret` never appears here -- only
/// [`resolve_for_call`] and the `/vault/callback` route ever decrypt it.
pub async fn providers(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_vault_providers(tenant.id).await?;
    Ok(json!({
        "providers": rows
            .into_iter()
            .map(|r| json!({
                "name": r.name,
                "auth_url": r.auth_url,
                "token_url": r.token_url,
                "client_id": r.client_id,
                "scopes": r.scopes,
                "created_unix": r.created_unix,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// `host.vault.status`'s own `end_user` resolution (P0 requirement 1,
/// AC1-AC2) -- deliberately not [`crate::enduser::resolve_end_user`]
/// itself: that helper's own `"self"`-with-no-identity error is
/// `end_user_required`, but AC2 pins this tool's own two failure codes to
/// `invalid_params` (omitted) and `upstream_not_connected` (`"self"` with
/// no identity on the call, since there is then no subject at all to
/// report status for -- the same code a resolved-but-unconnected call
/// already returns). A literal subject string (the tenant's own agent
/// reading a named end user, no identity required) and `"self"` with a
/// verified identity both still resolve exactly as
/// [`crate::enduser::resolve_end_user`] already does for `connect_link`.
fn resolve_status_end_user(args: &Value, end_user: Option<&EndUser>) -> Result<String, AppError> {
    match args.get("end_user") {
        None | Some(Value::Null) => Err(AppError::InvalidParams(
            "end_user is required: pass \"self\" or an end-user subject".to_string(),
        )),
        Some(Value::String(s)) if s == "self" => match end_user {
            Some(eu) => Ok(eu.subject.clone()),
            None => Err(AppError::Structured {
                code: "upstream_not_connected",
                message: "end_user: \"self\" has no verified end-user identity on this call".to_string(),
                data: json!({}),
            }),
        },
        Some(Value::String(_)) => {
            let (subject, _impersonated) = crate::enduser::resolve_end_user(args, end_user)?;
            Ok(subject)
        }
        Some(_) => Err(AppError::InvalidArgs(
            "end_user must be a string (\"self\" or a subject) or null".to_string(),
        )),
    }
}

/// `host.vault.status` (P0 requirement 1, AC1-AC3; AC6): the union of
/// every provider the tenant currently registers AND every provider this
/// specific end user has a `vault_tokens` row for -- AC6's own "each
/// user's `host.vault.status` shows `connected: false` with [the
/// `provider_removed`] reason" needs a just-removed provider (gone from
/// [`providers`]) to still surface here for a subject who has a residual
/// row, so this can't be driven by the registered-providers list alone.
/// Never reads the encrypted `access_enc`/`refresh_enc` columns
/// themselves, only [`crate::db::VaultTokenRow`]'s other fields.
pub async fn status(state: &AppState, tenant: &Tenant, args: &Value, end_user: Option<&EndUser>) -> Result<Value, AppError> {
    let subject = resolve_status_end_user(args, end_user)?;
    let registered = state.db.list_vault_provider_names(tenant.id).await?;
    let token_providers = state
        .db
        .list_vault_token_provider_names(tenant.id, subject.clone())
        .await?;
    let mut names: std::collections::BTreeSet<String> = registered.into_iter().collect();
    names.extend(token_providers);

    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let row = state.db.get_vault_token(tenant.id, name.clone(), subject.clone()).await?;
        out.push(match row {
            Some(r) => json!({
                "name": name,
                "connected": r.revoked_unix.is_none(),
                "expires_at": r.expires_unix,
                "scopes": r.scopes,
                "connected_at": r.connected_unix,
                "last_refreshed_at": r.last_refreshed_unix,
                "revoked_at": r.revoked_unix,
                "revoked_reason": r.revoked_reason,
            }),
            None => json!({
                "name": name,
                "connected": false,
                "expires_at": Value::Null,
                "scopes": Value::Null,
                "connected_at": Value::Null,
                "last_refreshed_at": Value::Null,
                "revoked_at": Value::Null,
                "revoked_reason": Value::Null,
            }),
        });
    }
    Ok(json!({"providers": out}))
}

/// `host.vault.provider_remove` (P0 requirement 3, AC6): the reverse of
/// `provider_set` -- deletes the provider and revokes every end user's
/// stored token for it in one transaction ([`crate::db::Db::remove_vault_provider`]).
pub async fn provider_remove(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let existed = state.db.remove_vault_provider(tenant.id, name.clone()).await?;
    if !existed {
        return Err(AppError::Structured {
            code: "not_found",
            message: format!("no vault provider named '{name}' is registered for this tenant"),
            data: json!({"name": name}),
        });
    }
    Ok(json!({"name": name, "removed": true}))
}

/// `host.vault.connect_link` (AC1): `end_user: "self"` is the only
/// supported shape (requirement 3 names no other) -- resolved through the
/// same [`crate::enduser::resolve_end_user`] every `host.state.*` op uses,
/// so a call with no verified end-user identity gets the same
/// `end_user_required` refusal it would from any of those.
pub async fn connect_link(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    let provider = arg_str(args, "provider")?;
    let (subject, impersonated) = crate::enduser::resolve_end_user(args, end_user)?;
    if subject.is_empty() {
        return Err(AppError::InvalidArgs(
            "end_user: \"self\" is required".to_string(),
        ));
    }
    let issuer = if impersonated { None } else { end_user.and_then(|e| e.issuer.clone()) };

    if state.db.get_vault_provider(tenant.id, provider.clone()).await?.is_none() {
        return Err(AppError::Structured {
            code: "vault_provider_not_found",
            message: format!("no vault provider named '{provider}' is registered for this tenant"),
            data: json!({"provider": provider}),
        });
    }

    let url = mint_connect_link(state, tenant, &provider, &subject, issuer.as_deref()).await?;
    Ok(json!({"connect_link": url}))
}

/// `host.vault.disconnect` (AC8): revokes locally; the provider's own
/// revocation URL (requirement 5's "when configured") is out of this
/// build's tested scope.
pub async fn disconnect(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    let provider = arg_str(args, "provider")?;
    let (subject, _impersonated) = crate::enduser::resolve_end_user(args, end_user)?;
    if subject.is_empty() {
        return Err(AppError::InvalidArgs(
            "end_user: \"self\" is required".to_string(),
        ));
    }
    state
        .db
        .revoke_vault_token(tenant.id, provider.clone(), subject, None)
        .await?;
    Ok(json!({"provider": provider, "disconnected": true}))
}

// ---- call-time resolution (requirement 4) ------------------------------

/// Mints a fresh one-time connect handoff (a new token, PKCE verifier and
/// anti-CSRF `state`, all stored via [`crate::db::Db::create_vault_handoff_token`])
/// and returns its `/vault/connect/<token>` URL -- shared by
/// [`connect_link`] and [`not_connected_error`] (AC6's "a fresh connect_link
/// URL"), so both give a caller the exact same working link.
async fn mint_connect_link(
    state: &AppState,
    tenant: &Tenant,
    provider: &str,
    subject: &str,
    issuer: Option<&str>,
) -> Result<String, AppError> {
    let token = generate_key();
    let token_hash = hash_key(&token);
    let verifier = generate_key();
    let (verifier_enc, verifier_nonce) = state.secrets.encrypt(&verifier)?;
    let oauth_state = generate_key();
    state
        .db
        .create_vault_handoff_token(
            tenant.id,
            token_hash,
            provider.to_string(),
            subject.to_string(),
            issuer.map(str::to_string),
            verifier_enc,
            verifier_nonce,
            oauth_state,
            now_unix() + HANDOFF_TTL_SECS,
        )
        .await?;
    Ok(format!(
        "{}/vault/connect/{}",
        state.public_url.trim_end_matches('/'),
        token
    ))
}

/// AC6: `upstream_not_connected`, carrying a fresh `connect_link` the
/// caller's agent can hand back to the human -- best-effort (a mint failure
/// just omits the field rather than masking the real refusal).
async fn not_connected_error(
    state: &AppState,
    tenant: &Tenant,
    provider: &str,
    subject: &str,
    issuer: Option<&str>,
) -> AppError {
    let mut data = json!({"provider": provider});
    if let Ok(link) = mint_connect_link(state, tenant, provider, subject, issuer).await
        && let Some(obj) = data.as_object_mut()
    {
        obj.insert("connect_link".to_string(), json!(link));
    }
    AppError::Structured {
        code: "upstream_not_connected",
        message: format!("no connected upstream token for provider '{provider}'"),
        data,
    }
}

/// requirement 4: resolved by `handler.rs::call_published_tool` before
/// `Kind::call` runs for any spec declaring `upstream_provider` -- refreshes
/// inline when the stored token is within [`REFRESH_WINDOW_SECS`] of expiry
/// (AC5), and marks a token that fails refresh `revoked_unix` with a reason
/// (P1 requirement 6 / AC10) so no later call on this connection ever
/// attempts another refresh.
pub async fn resolve_for_call(
    state: &AppState,
    tenant: &Tenant,
    provider: &str,
    end_user: Option<&EndUser>,
) -> Result<String, AppError> {
    let Some(eu) = end_user else {
        return Err(AppError::Structured {
            code: "upstream_not_connected",
            message: format!("no verified end-user identity on this call for provider '{provider}'"),
            data: json!({"provider": provider}),
        });
    };
    let subject = eu.subject.clone();
    let issuer = eu.issuer.clone();

    let Some(row) = state.db.get_vault_token(tenant.id, provider.to_string(), subject.clone()).await? else {
        return Err(not_connected_error(state, tenant, provider, &subject, issuer.as_deref()).await);
    };
    if row.revoked_unix.is_some() {
        return Err(not_connected_error(state, tenant, provider, &subject, issuer.as_deref()).await);
    }

    let now = now_unix();
    if row.expires_unix - now > REFRESH_WINDOW_SECS {
        return state.secrets.decrypt(&row.access_enc, &row.access_nonce);
    }

    let Some(refresh_enc) = row.refresh_enc.clone() else {
        if row.expires_unix > now {
            return state.secrets.decrypt(&row.access_enc, &row.access_nonce);
        }
        return Err(not_connected_error(state, tenant, provider, &subject, issuer.as_deref()).await);
    };
    let refresh_nonce = row.refresh_nonce.clone().unwrap_or_default();
    let refresh_token = state.secrets.decrypt(&refresh_enc, &refresh_nonce)?;

    let Some(provider_row) = state.db.get_vault_provider(tenant.id, provider.to_string()).await? else {
        return Err(not_connected_error(state, tenant, provider, &subject, issuer.as_deref()).await);
    };
    let client_secret = state
        .secrets
        .decrypt(&provider_row.client_secret_enc, &provider_row.client_secret_nonce)?;

    let send_result = state
        .http_client
        .post(&provider_row.token_url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", provider_row.client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ])
        .send()
        .await;

    let response = match send_result {
        Ok(r) => r,
        // A transport-level failure (the provider is unreachable) is
        // transient -- report not-connected for this call without burning
        // the stored token permanently, unlike an explicit 400/401 below.
        Err(_) => return Err(not_connected_error(state, tenant, provider, &subject, issuer.as_deref()).await),
    };

    if !response.status().is_success() {
        // P1 requirement 6 / AC10: the provider itself rejected the
        // refresh (400/401) -- revoked permanently, reason recorded.
        let reason = format!("refresh_http_{}", response.status().as_u16());
        state
            .db
            .revoke_vault_token(tenant.id, provider.to_string(), subject.clone(), Some(reason))
            .await?;
        return Err(not_connected_error(state, tenant, provider, &subject, issuer.as_deref()).await);
    }

    let body: Value = response.json().await.unwrap_or(Value::Null);
    let Some(access_token) = body.get("access_token").and_then(Value::as_str).map(str::to_string) else {
        state
            .db
            .revoke_vault_token(
                tenant.id,
                provider.to_string(),
                subject.clone(),
                Some("refresh_malformed_response".to_string()),
            )
            .await?;
        return Err(not_connected_error(state, tenant, provider, &subject, issuer.as_deref()).await);
    };
    let new_refresh_token = body
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or(refresh_token);
    let expires_in = body.get("expires_in").and_then(Value::as_i64).unwrap_or(3600);

    let (access_enc, access_nonce) = state.secrets.encrypt(&access_token)?;
    let (new_refresh_enc, new_refresh_nonce) = state.secrets.encrypt(&new_refresh_token)?;
    state
        .db
        .update_vault_token_refreshed(
            tenant.id,
            provider.to_string(),
            subject,
            access_enc,
            access_nonce,
            Some(new_refresh_enc),
            Some(new_refresh_nonce),
            now + expires_in,
        )
        .await?;

    Ok(access_token)
}

// ---- HTTP routes (requirement 3) ---------------------------------------

fn html_response(status: StatusCode, body: String) -> Response {
    (status, [("content-type", "text/html; charset=utf-8")], body).into_response()
}

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>{title}</title></head><body>{body}</body></html>"
    )
}

fn render_expired() -> String {
    page(
        "mcphost — link expired",
        "<h1>This link has expired or was already used</h1>",
    )
}

/// AC2: the literal phrase the acceptance criterion names, with no token
/// text anywhere on the page.
fn render_connected() -> String {
    page(
        "mcphost — connected",
        "<h1>Connected</h1><p>Return to your agent.</p>",
    )
}

fn render_storage_error() -> String {
    page(
        "mcphost — storage error",
        "<h1>Something went wrong</h1><p>Please try again in a minute.</p>",
    )
}

fn render_upstream_error() -> String {
    page(
        "mcphost — could not connect",
        "<h1>Could not connect</h1><p>Please ask your agent to try again.</p>",
    )
}

/// `application/x-www-form-urlencoded`-shaped percent-encoding for a query
/// value -- this crate has no `url`/`percent-encoding` dependency (the same
/// "no new dependency for a small, self-contained thing" call
/// `state::rfc3339_from_unix`'s hand-rolled calendar math already makes).
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

/// PKCE S256: `BASE64URL-ENCODE(SHA256(ASCII(code_verifier)))`, no padding
/// (RFC 7636 section 4.2).
fn pkce_challenge_s256(verifier: &str) -> String {
    use base64::Engine as _;
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// `GET /vault/connect/{token}` (requirement 3, AC1, AC7): the redeem
/// itself is the single-use gate (same atomic-claim shape as `claim.rs`'s
/// own handoff redemption) -- a second `GET` on the same token sees
/// [`VaultHandoffOutcome::AlreadyRedeemed`] and never starts a second
/// authorization.
pub async fn get_connect(State(state): State<Arc<AppState>>, Path(token): Path<String>) -> Response {
    let token_hash = hash_key(&token);
    let row = match state.db.redeem_vault_handoff_token(token_hash).await {
        Ok(VaultHandoffOutcome::Redeemed(row)) => row,
        Ok(VaultHandoffOutcome::NotFound | VaultHandoffOutcome::Expired | VaultHandoffOutcome::AlreadyRedeemed) => {
            return html_response(StatusCode::GONE, render_expired());
        }
        Err(_) => return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
    };
    let Ok(Some(provider_row)) = state.db.get_vault_provider(row.tenant_id, row.provider.clone()).await else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let Ok(verifier) = state.secrets.decrypt(&row.pkce_verifier_enc, &row.pkce_verifier_nonce) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };

    let challenge = pkce_challenge_s256(&verifier);
    let redirect_uri = format!("{}/vault/callback", state.public_url.trim_end_matches('/'));
    let query = format!(
        "client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        percent_encode(&provider_row.client_id),
        percent_encode(&redirect_uri),
        percent_encode(&provider_row.scopes),
        percent_encode(&row.oauth_state),
        percent_encode(&challenge),
    );
    let sep = if provider_row.auth_url.contains('?') { '&' } else { '?' };
    let location = format!("{}{sep}{query}", provider_row.auth_url);

    let Ok(value) = HeaderValue::from_str(&location) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    match Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, value)
        .body(Body::empty())
    {
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
}

/// `GET /vault/callback` (requirement 3, AC2): exchanges the code
/// server-side (never in the browser) and stores the resulting tokens
/// encrypted, keyed by whichever handoff row's `oauth_state` matches.
pub async fn get_callback(State(state): State<Arc<AppState>>, Query(q): Query<CallbackQuery>) -> Response {
    let (Some(code), Some(oauth_state)) = (q.code, q.state) else {
        return html_response(StatusCode::BAD_REQUEST, render_upstream_error());
    };
    let row = match state.db.find_vault_handoff_by_oauth_state(oauth_state).await {
        Ok(Some(row)) => row,
        Ok(None) => return html_response(StatusCode::GONE, render_expired()),
        Err(_) => return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
    };
    if now_unix() > row.expires_unix {
        return html_response(StatusCode::GONE, render_expired());
    }
    let Ok(Some(provider_row)) = state.db.get_vault_provider(row.tenant_id, row.provider.clone()).await else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let Ok(verifier) = state.secrets.decrypt(&row.pkce_verifier_enc, &row.pkce_verifier_nonce) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let Ok(client_secret) = state
        .secrets
        .decrypt(&provider_row.client_secret_enc, &provider_row.client_secret_nonce)
    else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let redirect_uri = format!("{}/vault/callback", state.public_url.trim_end_matches('/'));

    let send_result = state
        .http_client
        .post(&provider_row.token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", provider_row.client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await;

    let response = match send_result {
        Ok(r) if r.status().is_success() => r,
        _ => return html_response(StatusCode::BAD_GATEWAY, render_upstream_error()),
    };

    let body: Value = response.json().await.unwrap_or(Value::Null);
    let Some(access_token) = body.get("access_token").and_then(Value::as_str).map(str::to_string) else {
        return html_response(StatusCode::BAD_GATEWAY, render_upstream_error());
    };
    let refresh_token = body.get("refresh_token").and_then(Value::as_str).map(str::to_string);
    let expires_in = body.get("expires_in").and_then(Value::as_i64).unwrap_or(3600);

    let Ok((access_enc, access_nonce)) = state.secrets.encrypt(&access_token) else {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    };
    let (refresh_enc, refresh_nonce) = match &refresh_token {
        Some(rt) => match state.secrets.encrypt(rt) {
            Ok((e, n)) => (Some(e), Some(n)),
            Err(_) => return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
        },
        None => (None, None),
    };

    if state
        .db
        .upsert_vault_token(
            row.tenant_id,
            row.provider.clone(),
            row.end_user_subject.clone(),
            row.end_user_issuer.clone(),
            access_enc,
            access_nonce,
            refresh_enc,
            refresh_nonce,
            now_unix() + expires_in,
            provider_row.scopes.clone(),
        )
        .await
        .is_err()
    {
        return html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error());
    }

    html_response(StatusCode::OK, render_connected())
}
