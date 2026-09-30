//! PRD-mcphost-oauth-demand-signal requirements 2-3: the one aggregate
//! admin healthz's `oauth` field ([`healthz_json`], cached
//! [`HEALTHZ_CACHE_TTL_SECS`]) and `admin.oauth.demand_stats` ([`admin_stats`],
//! always fresh -- an operator tool, not a hot path) both build from, so
//! the two surfaces can never silently disagree on what a "call" or a
//! "tenant" counted here means.

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::state::AppState;

/// requirement 2: the two windows every count here is reported over.
pub const WINDOW_7D_SECS: i64 = 7 * 86_400;
/// PRD-mcphost-session-bound-tenant-after-signup requirement 8 (AC11):
/// `session_bound_calls_24h`'s own window -- the PRD names 24 h explicitly,
/// not this module's pre-existing 7d/30d pair.
pub const WINDOW_24H_SECS: i64 = 86_400;
pub const WINDOW_30D_SECS: i64 = 30 * 86_400;

/// requirement 2: `{key, issuer_jwt, hosted_token}` -- `calls.auth_method`'s
/// own domain (migration 0053), never absent even when a method has never
/// been used (`0`, not a missing key).
#[derive(Debug, Clone, Copy, Default)]
pub struct MethodCounts {
    pub key: i64,
    pub issuer_jwt: i64,
    pub hosted_token: i64,
}

impl MethodCounts {
    fn to_json(self) -> Value {
        json!({"key": self.key, "issuer_jwt": self.issuer_jwt, "hosted_token": self.hosted_token})
    }
}

/// requirement 2: admin healthz's whole `oauth` object.
#[derive(Debug, Clone, Default)]
pub struct HealthzAggregate {
    pub calls_7d: MethodCounts,
    pub calls_30d: MethodCounts,
    /// non-synthetic distinct tenants, `(issuer_jwt, hosted_token)`.
    pub tenants_7d: (i64, i64),
    pub tenants_30d: (i64, i64),
    pub clients_cimd: i64,
    pub clients_dcr: i64,
    pub grants_active: i64,
    pub first_issuer_jwt_call_at: Option<i64>,
    pub first_hosted_token_call_at: Option<i64>,
    /// PRD-mcphost-session-bound-tenant-after-signup requirement 8 (AC11):
    /// calls served by a post-`signup` session binding in the last 24 h, so
    /// an operator can see the feature is actually used.
    pub session_bound_calls_24h: i64,
}

impl HealthzAggregate {
    pub fn to_json(&self) -> Value {
        json!({
            "calls_7d": self.calls_7d.to_json(),
            "calls_30d": self.calls_30d.to_json(),
            "tenants_7d": {"issuer_jwt": self.tenants_7d.0, "hosted_token": self.tenants_7d.1},
            "tenants_30d": {"issuer_jwt": self.tenants_30d.0, "hosted_token": self.tenants_30d.1},
            "clients": {"cimd": self.clients_cimd, "dcr": self.clients_dcr},
            "grants_active": self.grants_active,
            "first_issuer_jwt_call_at": self.first_issuer_jwt_call_at,
            "first_hosted_token_call_at": self.first_hosted_token_call_at,
            "session_bound_calls_24h": self.session_bound_calls_24h,
        })
    }
}

/// requirement 2: one indexed query per window (technical considerations)
/// -- [`crate::db::Db::oauth_calls_by_method`]/[`crate::db::Db::oauth_tenants_by_method`]
/// each scan `calls` once, keyed off migration 0053's `(auth_method,
/// started_unix)`/`(tenant_id, auth_method, started_unix)` indexes.
pub async fn compute(state: &AppState) -> Result<HealthzAggregate, AppError> {
    let now = crate::state::now_unix();
    let (k7, j7, h7) = state.db.oauth_calls_by_method(now - WINDOW_7D_SECS).await?;
    let (k30, j30, h30) = state.db.oauth_calls_by_method(now - WINDOW_30D_SECS).await?;
    let tenants_7d = state.db.oauth_tenants_by_method(now - WINDOW_7D_SECS).await?;
    let tenants_30d = state.db.oauth_tenants_by_method(now - WINDOW_30D_SECS).await?;
    let (clients_cimd, clients_dcr) = state.db.oauth_clients_counts().await?;
    let grants_active = state.db.oauth_grants_active_count().await?;
    let first_issuer_jwt_call_at = state.db.oauth_first_call_at("issuer_jwt").await?;
    let first_hosted_token_call_at = state.db.oauth_first_call_at("hosted_token").await?;
    let session_bound_calls_24h = state.db.session_bound_calls_since(now - WINDOW_24H_SECS).await?;
    Ok(HealthzAggregate {
        calls_7d: MethodCounts { key: k7, issuer_jwt: j7, hosted_token: h7 },
        calls_30d: MethodCounts { key: k30, issuer_jwt: j30, hosted_token: h30 },
        tenants_7d,
        tenants_30d,
        clients_cimd,
        clients_dcr,
        grants_active,
        first_issuer_jwt_call_at,
        first_hosted_token_call_at,
        session_bound_calls_24h,
    })
}

/// requirement 2 (AC5): how long `/healthz` trusts a previously-computed
/// aggregate before recomputing -- non-functional: "healthz p95 unchanged."
pub const HEALTHZ_CACHE_TTL_SECS: i64 = 60;

/// One cached [`compute`] result -- the JSON value (so a cache hit costs no
/// re-serialization) and the unix time it was computed at.
#[derive(Debug, Clone)]
pub struct CachedHealthz {
    pub value: Value,
    pub fetched_at: i64,
}

/// The type [`AppState::oauth_healthz_cache`](crate::state::AppState::oauth_healthz_cache)
/// holds -- a single slot (unlike [`crate::billing::AcceptedUsageCache`]'s
/// per-tenant map): this aggregate is host-wide, not per-tenant, so there is
/// only ever one entry to cache. Same in-memory-only, `Arc`-shared-across-clones
/// rationale as every other in-process cache in this crate
/// ([`crate::billing::CheckoutSessionCache`]'s own doc comment).
pub type HealthzCache = std::sync::Arc<std::sync::Mutex<Option<CachedHealthz>>>;

/// requirement 2 (AC5): `/healthz`'s own `oauth` field -- serves the cached
/// aggregate when it's under [`HEALTHZ_CACHE_TTL_SECS`] old, else recomputes
/// and refreshes the cache. A storage error (e.g. AC14's unwritable
/// database) falls back to an all-zero aggregate rather than failing the
/// whole `/healthz` response over one additive field.
pub async fn healthz_json(state: &AppState) -> Value {
    let now = crate::state::now_unix();
    if let Ok(guard) = state.oauth_healthz_cache.lock()
        && let Some(cached) = guard.as_ref()
        && now - cached.fetched_at < HEALTHZ_CACHE_TTL_SECS
    {
        return cached.value.clone();
    }
    let value = match compute(state).await {
        Ok(agg) => agg.to_json(),
        Err(_) => HealthzAggregate::default().to_json(),
    };
    if let Ok(mut guard) = state.oauth_healthz_cache.lock() {
        *guard = Some(CachedHealthz { value: value.clone(), fetched_at: now });
    }
    value
}

/// requirement 3 (AC3/AC4): `admin.oauth.demand_stats` -- the same aggregate as
/// [`healthz_json`] (recomputed fresh; an operator tool, not a request-path
/// hot spot, so it skips the cache entirely) plus per-tenant rows and the
/// registration/consent/token funnel.
pub async fn admin_stats(state: &AppState) -> Result<Value, AppError> {
    let agg = compute(state).await?;
    let now = crate::state::now_unix();
    let since_7d = now - WINDOW_7D_SECS;
    let tenants = state.db.oauth_tenant_stats(since_7d).await?;
    let (dcr_7d, cimd_7d, authorize_requests_7d, consents_7d, tokens_issued_7d) =
        state.db.oauth_funnel_counts(since_7d).await?;

    let mut body = agg.to_json();
    let obj = body.as_object_mut().expect("to_json always returns an object");
    obj.insert(
        "tenants".to_string(),
        json!(
            tenants
                .into_iter()
                .map(|t| {
                    json!({
                        "tenant_id": t.tenant_id,
                        "synthetic": t.synthetic,
                        "calls_7d": {
                            "key": t.calls_key,
                            "issuer_jwt": t.calls_issuer_jwt,
                            "hosted_token": t.calls_hosted_token,
                        },
                        "last_oauth_call_at": t.last_oauth_call_at,
                    })
                })
                .collect::<Vec<_>>()
        ),
    );
    obj.insert(
        "funnel".to_string(),
        json!({
            "registrations_7d": {"dcr": dcr_7d, "cimd": cimd_7d},
            "authorize_requests_7d": authorize_requests_7d,
            "consents_7d": consents_7d,
            "tokens_issued_7d": tokens_issued_7d,
        }),
    );
    Ok(body)
}
