//! PRD-mcphost-oauth-resource-server: RFC 9728 protected-resource metadata,
//! tenant-registered OAuth issuers, and bearer-JWT validation against a
//! per-issuer JWKS -- `resource_metadata`/`bearer` glue lives in `http.rs`
//! (the `/.well-known/oauth-protected-resource` route and the 401 +
//! `WWW-Authenticate` upgrade); this module owns the tenant/admin tool
//! business logic and the JWT/JWKS mechanics `handler::resolve_auth` calls
//! into.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::db::{OauthIssuerRow, Tenant};
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// requirement 3: "one tenant may register up to 3 issuers".
pub const MAX_ISSUERS_PER_TENANT: i64 = 3;
/// requirement 4: "through the host's own client with 5 s timeout".
const JWKS_FETCH_TIMEOUT_SECS: u64 = 5;
/// requirement 4 / AC5: "refetch on unknown `kid` at most once per 60 s per
/// issuer".
const UNKNOWN_KID_REFETCH_COOLDOWN_SECS: u64 = 60;
/// requirement 4: "`exp`/`nbf` with 60 s skew".
const CLOCK_SKEW_SECS: i64 = 60;
/// P2 requirement 8 default.
pub const DEFAULT_ALLOWED_ALGS: &str = "RS256,ES256";
/// P2 requirement 8 default.
pub const DEFAULT_JWKS_TTL_SECS: i64 = 3600;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

/// PRD-mcphost-tenant-resource-metadata requirement 1/2: the one resource
/// URI a tenant's namespace maps to -- computed from `public_url` and the
/// namespace every time, never stored, so it can never drift from either.
pub fn canonical_resource_uri(public_url: &str, namespace: &str) -> String {
    format!("{public_url}/t/{namespace}/mcp")
}

/// requirement 2: the RFC 9728 metadata document URL for a tenant's own
/// canonical resource URI above.
pub fn tenant_metadata_url(public_url: &str, namespace: &str) -> String {
    format!("{public_url}/.well-known/oauth-protected-resource/t/{namespace}/mcp")
}

/// PRD-mcphost-tool-scopes-and-consent requirement 2: the inverse of
/// [`canonical_resource_uri`] -- `/oauth/authorize`'s own `resource`
/// parameter names which tenant's scope catalog governs consent, before
/// that tenant is even authenticated (the URL itself already names it, the
/// same information a `/t/{ns}/mcp` path segment already exposes). `None`
/// for anything that isn't shaped like a per-tenant resource URI at all
/// (the root resource, or garbage).
pub fn resource_tenant_namespace(public_url: &str, resource: &str) -> Option<String> {
    let prefix = format!("{}/t/", public_url.trim_end_matches('/'));
    resource
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix("/mcp"))
        .filter(|ns| !ns.is_empty())
        .map(str::to_string)
}

/// P2 requirement 8: `$MCPHOST_OAUTH_ALLOWED_ALGS`, a comma-separated list
/// of `RS256`/`ES256` (the only two this crate's JWK-to-`DecodingKey`
/// mapping in [`decoding_key_for`] can ever produce); an unrecognized name
/// is silently dropped rather than failing startup, same "warn and fall
/// back" posture as [`crate::state::parse_signup_rate_limit_per_hour`].
pub fn parse_allowed_algs(raw: Option<&str>) -> Vec<Algorithm> {
    let raw = raw.unwrap_or(DEFAULT_ALLOWED_ALGS);
    let algs: Vec<Algorithm> = raw
        .split(',')
        .filter_map(|s| match s.trim() {
            "RS256" => Some(Algorithm::RS256),
            "ES256" => Some(Algorithm::ES256),
            _ => None,
        })
        .collect();
    if algs.is_empty() {
        vec![Algorithm::RS256, Algorithm::ES256]
    } else {
        algs
    }
}

pub fn allowed_algs_from_env() -> Vec<Algorithm> {
    let raw = std::env::var("MCPHOST_OAUTH_ALLOWED_ALGS").ok();
    parse_allowed_algs(raw.as_deref())
}

/// P2 requirement 8: `$MCPHOST_OAUTH_JWKS_TTL_SECS`.
pub fn parse_jwks_ttl_secs(raw: Option<&str>) -> i64 {
    match raw {
        None => DEFAULT_JWKS_TTL_SECS,
        Some(raw) => raw.parse::<i64>().unwrap_or(DEFAULT_JWKS_TTL_SECS),
    }
}

pub fn jwks_ttl_secs_from_env() -> i64 {
    let raw = std::env::var("MCPHOST_OAUTH_JWKS_TTL_SECS").ok();
    parse_jwks_ttl_secs(raw.as_deref())
}

// ---- JWK JSON shape (hand-rolled, not jsonwebtoken::jwk) -----------------
//
// Only the fields `decoding_key_for` needs from an RSA or EC public key --
// deliberately not `jsonwebtoken::jwk`'s own types (this pinned crate
// version's availability of that module is unverified, and every field
// this crate actually reads is a plain string already base64url-encoded
// exactly as `DecodingKey::from_rsa_components`/`from_ec_components`
// expect, so hand-mapping the JSON is a few lines, not a parser). The
// actual cryptographic signature verification still goes entirely through
// `jsonwebtoken`'s audited `DecodingKey`/`decode`.

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JwkDoc {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JwkSetDoc {
    keys: Vec<JwkDoc>,
}

fn decoding_key_for(jwk: &JwkDoc) -> Option<(DecodingKey, Algorithm)> {
    match jwk.kty.as_str() {
        "RSA" => {
            let key = DecodingKey::from_rsa_components(jwk.n.as_deref()?, jwk.e.as_deref()?).ok()?;
            Some((key, Algorithm::RS256))
        }
        "EC" if jwk.crv.as_deref() == Some("P-256") => {
            let key = DecodingKey::from_ec_components(jwk.x.as_deref()?, jwk.y.as_deref()?).ok()?;
            Some((key, Algorithm::ES256))
        }
        _ => None,
    }
}

fn find_kid<'a>(keys: &'a [JwkDoc], kid: Option<&str>) -> Option<&'a JwkDoc> {
    match kid {
        Some(k) => keys.iter().find(|j| j.kid.as_deref() == Some(k)),
        // A JWT with no `kid` header: fall back to the sole key, the common
        // shape for a single-key JWKS deployment.
        None if keys.len() == 1 => keys.first(),
        None => None,
    }
}

async fn fetch_jwks(state: &AppState, url: &str) -> Option<Vec<JwkDoc>> {
    let resp = state
        .http_client
        .get(url)
        .timeout(Duration::from_secs(JWKS_FETCH_TIMEOUT_SECS))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let doc: JwkSetDoc = resp.json().await.ok()?;
    Some(doc.keys)
}

struct CachedJwks {
    keys: Vec<JwkDoc>,
    fetched_at: Instant,
    /// `true` once at least one fetch for this issuer has ever succeeded --
    /// distinguishes "we have a JWKS, this kid just isn't in it"
    /// (`bad_signature`) from "we have never once reached this issuer's
    /// JWKS endpoint" (`unknown_issuer`, AC6), which an empty-but-fetched
    /// `keys` (an operator's genuinely empty JWKS) must not be confused
    /// with.
    ever_fetched: bool,
    /// AC5's 60s-per-issuer throttle on an unknown-`kid`-triggered refetch.
    last_unknown_kid_refetch: Option<Instant>,
}

/// [`crate::state::AppState::oauth`]: the in-process JWKS cache, keyed by
/// issuer URL string (globally unique, migration 0038's UNIQUE constraint)
/// -- shared, `Arc`-wrapped mutable state, same convention as
/// [`crate::bans::BanCache`].
#[derive(Clone, Default)]
pub struct JwksCache {
    inner: Arc<Mutex<HashMap<String, CachedJwks>>>,
}

/// [`JwksCache::resolve_key`]'s outcome, distinguishing the two rejection
/// reasons a caller with no matching key can hit.
enum KeyResolution {
    Found(JwkDoc),
    /// AC6: no cached JWKS, and this attempt (or a prior one) never
    /// obtained one -> `unknown_issuer`.
    NeverCached,
    /// AC5: a real cached JWKS exists, but this `kid` isn't (or isn't yet
    /// again) in it -> `bad_signature`.
    KidNotFound,
}

impl JwksCache {
    pub fn new() -> Self {
        Self::default()
    }

    async fn store(&self, state: &AppState, issuer: &str, keys: Vec<JwkDoc>) {
        {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let entry = guard.entry(issuer.to_string()).or_insert_with(|| CachedJwks {
                keys: Vec::new(),
                fetched_at: Instant::now(),
                ever_fetched: false,
                last_unknown_kid_refetch: None,
            });
            entry.keys = keys.clone();
            entry.fetched_at = Instant::now();
            entry.ever_fetched = true;
        }
        if let Ok(json) = serde_json::to_string(&JwkSetDoc { keys }) {
            let _ = state.db.update_oauth_issuer_jwks(issuer.to_string(), json, now_unix()).await;
        }
    }

    fn stamp_unknown_kid_refetch(&self, issuer: &str) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.entry(issuer.to_string()).or_insert_with(|| CachedJwks {
            keys: Vec::new(),
            fetched_at: Instant::now(),
            ever_fetched: false,
            last_unknown_kid_refetch: None,
        });
        entry.last_unknown_kid_refetch = Some(Instant::now());
    }

    /// The one entry point [`validate_bearer`] resolves a `kid` through.
    /// TTL-stale (or altogether missing) cache triggers a proactive
    /// refetch first (not throttled -- a distinct trigger from the
    /// unknown-`kid` cooldown below); a `kid` still not found after that
    /// falls to the AC5 throttle: refetch once, then withhold further
    /// network calls for [`UNKNOWN_KID_REFETCH_COOLDOWN_SECS`] regardless
    /// of how many more unknown `kid`s arrive for this issuer in that
    /// window.
    #[allow(clippy::too_many_arguments)]
    async fn resolve_key(
        &self,
        state: &AppState,
        issuer: &str,
        jwks_url: &str,
        kid: Option<&str>,
        ttl_secs: i64,
    ) -> KeyResolution {
        let ttl = Duration::from_secs(ttl_secs.max(1) as u64);
        let stale_or_missing = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            match guard.get(issuer) {
                None => true,
                Some(c) => Instant::now().duration_since(c.fetched_at) >= ttl,
            }
        };
        if stale_or_missing
            && let Some(keys) = fetch_jwks(state, jwks_url).await
        {
            self.store(state, issuer, keys).await;
        }

        if let Some(jwk) = self.cached_kid(issuer, kid) {
            return KeyResolution::Found(jwk);
        }

        let ever_fetched = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.get(issuer).is_some_and(|c| c.ever_fetched)
        };
        if !ever_fetched {
            return KeyResolution::NeverCached;
        }

        let should_refetch = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.get(issuer).is_none_or(|c| {
                c.last_unknown_kid_refetch.is_none_or(|t| {
                    Instant::now().duration_since(t)
                        >= Duration::from_secs(UNKNOWN_KID_REFETCH_COOLDOWN_SECS)
                })
            })
        };
        if !should_refetch {
            return KeyResolution::KidNotFound;
        }
        self.stamp_unknown_kid_refetch(issuer);
        if let Some(keys) = fetch_jwks(state, jwks_url).await {
            self.store(state, issuer, keys).await;
        }

        match self.cached_kid(issuer, kid) {
            Some(jwk) => KeyResolution::Found(jwk),
            None => KeyResolution::KidNotFound,
        }
    }

    fn cached_kid(&self, issuer: &str, kid: Option<&str>) -> Option<JwkDoc> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(issuer).and_then(|c| find_kid(&c.keys, kid).cloned())
    }

    /// `admin.oauth.jwks_refresh`: an unconditional fetch, bypassing both
    /// the TTL and the unknown-`kid` throttle -- an operator's explicit
    /// override. `true` iff the fetch succeeded.
    pub async fn force_refresh(&self, state: &AppState, issuer: &str, jwks_url: &str) -> bool {
        match fetch_jwks(state, jwks_url).await {
            Some(keys) => {
                self.store(state, issuer, keys).await;
                true
            }
            None => false,
        }
    }
}

/// [`validate_bearer`]'s success shape.
pub struct OauthCaller {
    pub tenant_id: i64,
    pub subject: String,
    /// PRD-mcphost-end-user-identity requirement 1: the registered
    /// issuer's own URL (`issuer_row.issuer`) -- the JWT's own `iss` claim
    /// re-read from the row a registered issuer already matched, not a
    /// second parse of the token.
    pub issuer: String,
    /// PRD-mcphost-hosted-authorization-server requirement 5: `"oauth"`
    /// for a tenant-registered bring-your-own issuer (unchanged from
    /// before this PRD), `"hosted_token"` for this host's own built-in
    /// issuer, PRD-mcphost-federated-end-user-login requirement 3:
    /// `"federated"` for an end user who logged in through a tenant's own
    /// OIDC provider -- `handler.rs` surfaces this as `host.whoami`'s
    /// `auth_method` and picks the matching `EndUserMethod`
    /// (`Oauth`/`HostedOauth`/`Federated`).
    pub auth_method: &'static str,
    /// PRD-mcphost-tenant-resource-metadata requirement 4 (AC5): the JWT's
    /// own `scope` claim, verbatim, when present -- `None` for a token that
    /// never carries the claim at all, which spec 2026-07-28 treats as
    /// unrestricted (never `insufficient_scope`), distinct from a present
    /// but empty/non-`mcp` scope string.
    pub scope: Option<String>,
    /// PRD-mcphost-federated-end-user-login requirement 3: `Some` only for
    /// `auth_method == "federated"`, when the provider supplied it --
    /// surfaced as `MCPHOST_END_USER_EMAIL`/`host.enduser.whoami`'s `email`.
    pub email: Option<String>,
    /// See [`OauthCaller::email`]; surfaced as `MCPHOST_END_USER_NAME`.
    pub name: Option<String>,
}

/// The JWT's `payload` segment, decoded (base64url) but NOT signature
/// verified -- used only to read `iss` before the issuer (and therefore
/// which JWKS/algorithm) is even known. Every claim this crate actually
/// trusts (`aud`/`exp`/`nbf`/`sub`) is re-read from the signature-verified
/// `jsonwebtoken::decode` output in [`validate_bearer`] below, never from
/// this pre-verification peek.
fn peek_claims(token: &str) -> Option<Value> {
    use base64::Engine as _;
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    parts.next()?; // signature segment must exist too (a real 3-part JWT)
    if parts.next().is_some() {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// requirement 4/5: bearer JWT validation against a tenant-registered
/// issuer's JWKS. Every early return that has identified a registered
/// issuer also records the rejection reason via
/// [`crate::db::Db::increment_oauth_rejection`] (AC3/AC9); the two cases
/// where no issuer could be identified at all (`unknown_issuer`,
/// `malformed` before `iss` is even readable) have nothing to key a
/// per-issuer counter on and record nothing, matching AC3's "same setup"
/// (a registered issuer) scoping the counter requirement to the cases that
/// actually have one.
pub async fn validate_bearer(state: &AppState, token: &str) -> Result<OauthCaller, AppError> {
    let header = jsonwebtoken::decode_header(token).map_err(|_| AppError::InvalidToken("malformed"))?;
    let claims_peek = peek_claims(token).ok_or(AppError::InvalidToken("malformed"))?;
    let iss = claims_peek
        .get("iss")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(AppError::InvalidToken("malformed"))?;

    // PRD-mcphost-hosted-authorization-server requirement 5: this host is
    // always its own built-in issuer for every tenant (no `oauth_issuers`
    // row) -- checked before the registered-issuer lookup below, which
    // would otherwise never match it (a tenant cannot `host.oauth.issuer_set`
    // this host's own URL onto itself).
    if iss.trim_end_matches('/') == state.public_url.trim_end_matches('/') {
        return crate::authz::validate_hosted_bearer(state, token, &header).await;
    }

    let issuer_row = state
        .db
        .find_oauth_issuer_by_issuer(iss)
        .await
        .unwrap_or(None)
        .ok_or(AppError::InvalidToken("unknown_issuer"))?;

    let resolution = state
        .oauth
        .resolve_key(
            state,
            &issuer_row.issuer,
            &issuer_row.jwks_url,
            header.kid.as_deref(),
            state.oauth_jwks_ttl_secs,
        )
        .await;
    let jwk = match resolution {
        KeyResolution::Found(jwk) => jwk,
        KeyResolution::NeverCached => return Err(AppError::InvalidToken("unknown_issuer")),
        KeyResolution::KidNotFound => {
            let _ = state
                .db
                .increment_oauth_rejection(issuer_row.issuer.clone(), "bad_signature".to_string())
                .await;
            return Err(AppError::InvalidToken("bad_signature"));
        }
    };

    let Some((decoding_key, alg)) = decoding_key_for(&jwk) else {
        let _ = state
            .db
            .increment_oauth_rejection(issuer_row.issuer.clone(), "bad_signature".to_string())
            .await;
        return Err(AppError::InvalidToken("bad_signature"));
    };
    if !state.oauth_allowed_algs.contains(&alg) {
        let _ = state
            .db
            .increment_oauth_rejection(issuer_row.issuer.clone(), "bad_signature".to_string())
            .await;
        return Err(AppError::InvalidToken("bad_signature"));
    }

    let mut validation = Validation::new(alg);
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();

    let token_data = match jsonwebtoken::decode::<Value>(token, &decoding_key, &validation) {
        Ok(data) => data,
        Err(_) => {
            let _ = state
                .db
                .increment_oauth_rejection(issuer_row.issuer.clone(), "bad_signature".to_string())
                .await;
            return Err(AppError::InvalidToken("bad_signature"));
        }
    };
    let claims = token_data.claims;

    // PRD-mcphost-tenant-resource-metadata requirement 3 (RFC 8707 §2): an
    // audience is acceptable when it's either this issuer's own registered
    // `audience` (unchanged -- AC7's existing suite pins this) or this
    // issuer's owning tenant's canonical resource URI, even when the
    // tenant registered a distinct custom audience. A missing/unknown
    // owner tenant is unreachable in practice (`issuer_row.tenant_id` is a
    // foreign key into `tenants`) but falls back to the registered
    // audience alone rather than panicking.
    let owner_canonical = state
        .db
        .find_tenant_by_id(issuer_row.tenant_id)
        .await
        .unwrap_or(None)
        .map(|t| canonical_resource_uri(&state.public_url, &t.namespace));
    let aud_matches = |aud: &str| -> bool {
        aud == issuer_row.audience || owner_canonical.as_deref() == Some(aud)
    };
    let aud_ok = match claims.get("aud") {
        Some(Value::String(s)) => aud_matches(s),
        Some(Value::Array(arr)) => arr.iter().any(|v| v.as_str().is_some_and(aud_matches)),
        _ => false,
    };
    if !aud_ok {
        let _ = state
            .db
            .increment_oauth_rejection(issuer_row.issuer.clone(), "wrong_audience".to_string())
            .await;
        return Err(AppError::InvalidToken("wrong_audience"));
    }

    let now = now_unix();
    let Some(exp) = claims.get("exp").and_then(Value::as_i64) else {
        let _ = state
            .db
            .increment_oauth_rejection(issuer_row.issuer.clone(), "malformed".to_string())
            .await;
        return Err(AppError::InvalidToken("malformed"));
    };
    if now > exp + CLOCK_SKEW_SECS {
        let _ = state
            .db
            .increment_oauth_rejection(issuer_row.issuer.clone(), "expired".to_string())
            .await;
        return Err(AppError::InvalidToken("expired"));
    }
    if let Some(nbf) = claims.get("nbf").and_then(Value::as_i64)
        && now + CLOCK_SKEW_SECS < nbf
    {
        let _ = state
            .db
            .increment_oauth_rejection(issuer_row.issuer.clone(), "expired".to_string())
            .await;
        return Err(AppError::InvalidToken("expired"));
    }

    let Some(sub) = claims.get("sub").and_then(Value::as_str).map(str::to_string) else {
        let _ = state
            .db
            .increment_oauth_rejection(issuer_row.issuer.clone(), "malformed".to_string())
            .await;
        return Err(AppError::InvalidToken("malformed"));
    };

    let scope = claims.get("scope").and_then(Value::as_str).map(str::to_string);
    Ok(OauthCaller {
        tenant_id: issuer_row.tenant_id,
        subject: sub,
        issuer: issuer_row.issuer,
        auth_method: "oauth",
        scope,
        email: None,
        name: None,
    })
}

// ---- host.oauth.* tenant tools --------------------------------------------

/// `host.oauth.issuer_set {issuer, audience, jwks_url}` (requirement 3):
/// re-registering an issuer this same tenant already owns updates
/// `audience`/`jwks_url` in place (idempotent); an issuer another tenant
/// owns is refused `issuer_already_registered`; a 4th distinct issuer is
/// refused `issuer_quota_exceeded`.
pub async fn issuer_set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let issuer = arg_str(args, "issuer")?;
    // PRD-mcphost-tenant-resource-metadata requirement 3: `audience` is now
    // optional, defaulting to this tenant's own canonical resource URI --
    // the value AC1's "issuer registered with no audience" Given relies on.
    let audience =
        arg_str_opt(args, "audience").unwrap_or_else(|| canonical_resource_uri(&state.public_url, &tenant.namespace));
    let jwks_url = arg_str(args, "jwks_url")?;

    if let Some(existing) = state.db.find_oauth_issuer_by_issuer(issuer.clone()).await? {
        if existing.tenant_id != tenant.id {
            return Err(AppError::IssuerAlreadyRegistered);
        }
        state
            .db
            .update_oauth_issuer(tenant.id, issuer.clone(), audience.clone(), jwks_url.clone())
            .await?;
        return Ok(json!({"issuer": issuer, "audience": audience, "jwks_url": jwks_url}));
    }

    let used = state.db.count_oauth_issuers_by_tenant(tenant.id).await?;
    if used >= MAX_ISSUERS_PER_TENANT {
        return Err(AppError::IssuerQuotaExceeded { limit: MAX_ISSUERS_PER_TENANT, used });
    }

    state
        .db
        .insert_oauth_issuer(
            tenant.id,
            issuer.clone(),
            audience.clone(),
            jwks_url.clone(),
            crate::state::rfc3339_now(),
        )
        .await?;
    Ok(json!({"issuer": issuer, "audience": audience, "jwks_url": jwks_url}))
}

/// `host.oauth.issuer_remove {issuer}`.
pub async fn issuer_remove(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let issuer = arg_str(args, "issuer")?;
    let removed = state.db.remove_oauth_issuer(tenant.id, issuer.clone()).await?;
    if !removed {
        return Err(AppError::IssuerNotFound(issuer));
    }
    Ok(json!({"removed": true, "issuer": issuer}))
}

fn issuer_row_json(row: &OauthIssuerRow) -> Value {
    json!({
        "issuer": row.issuer,
        "audience": row.audience,
        "jwks_url": row.jwks_url,
        "created_at": row.created_at,
        "last_jwks_at": row.last_jwks_at,
    })
}

/// `host.oauth.issuers`: this tenant's own registered issuers.
pub async fn issuers_list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_oauth_issuers_by_tenant(tenant.id).await?;
    let issuers: Vec<Value> = rows.iter().map(issuer_row_json).collect();
    Ok(json!({"issuers": issuers}))
}

// ---- oauth_scopes: a tenant's own scope catalog (PRD-mcphost-tool-scopes-
// and-consent requirement 1) ------------------------------------------------

/// requirement 1: "a tool may reference only catalogued scopes or the
/// built-ins `read`, `write`" -- these two need no `oauth_scopes` row to be
/// valid on a tool's own `scopes`; a catalog entry only ever adds a
/// consent-page description for them (or names an entirely custom scope).
pub const BUILTIN_SCOPES: [&str; 2] = ["read", "write"];

/// requirement 1: 32 catalog entries per tenant.
pub const MAX_SCOPES_PER_TENANT: i64 = 32;

/// Every scope name valid for this tenant to use anywhere -- on a
/// `/oauth/authorize` request, on a tool's own `scopes`, or listed in a
/// metadata document's `scopes_supported` (AC7): `mcp` (the hierarchy root),
/// the two built-ins, and this tenant's own catalog names.
pub fn known_scope_names(catalog: &[crate::db::OauthScopeRow]) -> std::collections::HashSet<String> {
    let mut set: std::collections::HashSet<String> =
        std::iter::once("mcp".to_string()).chain(BUILTIN_SCOPES.iter().map(|s| s.to_string())).collect();
    set.extend(catalog.iter().map(|s| s.name.clone()));
    set
}

/// requirement 1: a tool's own declared `scopes`, defaulted -- empty means
/// "requires only `mcp`" (never an unsatisfiable empty requirement).
fn required_or_mcp(scopes: &[String]) -> Vec<String> {
    if scopes.is_empty() { vec!["mcp".to_string()] } else { scopes.to_vec() }
}

/// requirements 3/4 (AC2, AC6): whether `token_scope` (an `OauthCaller`'s
/// own `scope` claim -- `None` for a key-based caller, or a token that
/// never carries the claim at all, both unrestricted) satisfies `required`
/// (a tool's own `scopes`) -- `mcp`'s hierarchy always satisfies every
/// scope, so a token carrying it needs no further check. The one gate both
/// `tools/list`'s per-row filter and `tools/call`'s dispatch-time refusal
/// share, so a scope change on republish (AC6) is read fresh by whichever
/// runs next, no cache to invalidate.
pub fn scope_satisfied(token_scope: Option<&str>, required: &[String]) -> bool {
    let Some(scope) = token_scope else {
        return true;
    };
    let words: std::collections::HashSet<&str> = scope.split_whitespace().collect();
    if words.contains("mcp") {
        return true;
    }
    required_or_mcp(required).iter().all(|r| words.contains(r.as_str()))
}

/// requirement 4 (AC2): the `insufficient_scope` challenge's own `scope`
/// value -- the union of `token_scope`'s recognized words (intersected
/// against `mcp` ∪ the built-ins ∪ this tenant's own catalog, so a foreign
/// word like `offline_access` or an unrelated scope never leaks in) and the
/// tool's own required scopes, sorted. A token with no `scope` claim at all
/// never reaches this (requirement 4/[`scope_satisfied`] never refuses it),
/// so `token_scope` here is always genuinely restrictive.
pub async fn union_scope_challenge(
    state: &AppState,
    tenant_id: i64,
    token_scope: Option<&str>,
    required: &[String],
) -> String {
    let catalog = state.db.list_oauth_scopes(tenant_id).await.unwrap_or_default();
    let known = known_scope_names(&catalog);
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(scope) = token_scope {
        for w in scope.split_whitespace() {
            if known.contains(w) {
                set.insert(w.to_string());
            }
        }
    }
    set.extend(required_or_mcp(required));
    set.into_iter().collect::<Vec<_>>().join(" ")
}

/// `host.oauth.scope_set {name, description}` (requirement 1): upserts one
/// catalog entry -- re-setting an already-owned `name` updates its
/// `description` in place and never counts a second time against the
/// 32-entry cap.
pub async fn scope_set(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    let description = arg_str(args, "description")?;
    crate::state::validate_scope_name(&name)?;
    let existing = state.db.list_oauth_scopes(tenant.id).await?;
    if !existing.iter().any(|s| s.name == name) {
        let used = state.db.count_oauth_scopes(tenant.id).await?;
        if used >= MAX_SCOPES_PER_TENANT {
            return Err(AppError::InvalidParams(format!(
                "scope catalog already holds {used} entries, the maximum {MAX_SCOPES_PER_TENANT}"
            )));
        }
    }
    state.db.upsert_oauth_scope(tenant.id, name.clone(), description.clone()).await?;
    Ok(json!({"name": name, "description": description}))
}

/// `host.oauth.scopes`: this tenant's own scope catalog.
pub async fn scopes_list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_oauth_scopes(tenant.id).await?;
    let scopes: Vec<Value> = rows
        .iter()
        .map(|r| json!({"name": r.name, "description": r.description}))
        .collect();
    Ok(json!({"scopes": scopes}))
}

// ---- admin.oauth.* -------------------------------------------------------

/// requirement 6 (AC8): 7 days, matching `consents_by_scope_7d`'s own name.
const CONSENTS_BY_SCOPE_WINDOW_SECS: i64 = 7 * 86_400;

/// `admin.oauth.stats` (requirement 6, AC8): `consents_by_scope_7d` tallies
/// every space-separated word across every grant (one per completed
/// consent -- `host.oauth.grants`' own unit) created in the last 7 days,
/// e.g. two `read`-only consents and one `read write` consent tally to
/// `{read: 3, write: 1}`.
pub async fn admin_stats(state: &AppState) -> Result<Value, AppError> {
    let since = now_unix() - CONSENTS_BY_SCOPE_WINDOW_SECS;
    let scopes = state.db.list_oauth_grant_scopes_since(since).await?;
    let mut tally: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for scope in &scopes {
        for word in scope.split_whitespace() {
            *tally.entry(word.to_string()).or_insert(0) += 1;
        }
    }
    Ok(json!({"consents_by_scope_7d": tally}))
}

/// `admin.oauth.issuers` (AC9): every registered issuer, its owning
/// tenant, JWKS age, and per-reason rejection counters.
pub async fn admin_issuers(state: &AppState) -> Result<Value, AppError> {
    let rows = state.db.list_oauth_issuers_with_tenant().await?;
    let mut issuers = Vec::with_capacity(rows.len());
    for (row, namespace) in rows {
        let counts = state.db.oauth_rejection_counts(row.issuer.clone()).await?;
        let mut rejections = Map::new();
        for (reason, count) in counts {
            rejections.insert(reason, json!(count));
        }
        let jwks_age_s = row.last_jwks_at.map(|t| (now_unix() - t).max(0));
        issuers.push(json!({
            "issuer": row.issuer,
            "tenant": namespace,
            "audience": row.audience,
            "jwks_url": row.jwks_url,
            "jwks_age_s": jwks_age_s,
            "rejections": Value::Object(rejections),
        }));
    }
    Ok(json!({"issuers": issuers}))
}

/// `admin.oauth.jwks_refresh {issuer}` (requirement 6): forces a refetch
/// regardless of TTL or the unknown-`kid` throttle.
pub async fn admin_jwks_refresh(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let issuer = arg_str(args, "issuer")?;
    let Some(row) = state.db.find_oauth_issuer_by_issuer(issuer.clone()).await? else {
        return Err(AppError::IssuerNotFound(issuer));
    };
    let refreshed = state.oauth.force_refresh(state, &row.issuer, &row.jwks_url).await;
    Ok(json!({"issuer": issuer, "refreshed": refreshed}))
}

// ---- RFC 9728 protected-resource metadata --------------------------------

/// `GET /.well-known/oauth-protected-resource` (AC1, requirement 1):
/// `resource` is this host's own public URL; `authorization_servers` is
/// the distinct issuer URLs registered across every tenant (never a
/// tenant id or namespace -- Technical considerations: "must not
/// enumerate tenants"), plus (PRD-mcphost-hosted-authorization-server AC1)
/// this host's own URL -- mcphost is now always an authorization server in
/// its own right, whether or not any tenant has registered a bring-your-own
/// issuer, so that entry is never conditional on `issuers` being non-empty.
/// Deduplicated (a bring-your-own issuer whose `issuer` happens to equal
/// this host's own `public_url` would otherwise double-list it) and sorted
/// so the host's own URL sorts wherever it falls lexicographically rather
/// than depending on insertion order.
pub async fn protected_resource_metadata(state: &AppState) -> Result<Value, AppError> {
    let mut issuers = state.db.list_distinct_oauth_issuer_urls().await?;
    let own = state.public_url.trim_end_matches('/').to_string();
    if !issuers.iter().any(|i| i == &own) {
        issuers.push(own);
    }
    issuers.sort();
    Ok(json!({
        "resource": state.public_url,
        "authorization_servers": issuers,
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["mcp"],
        "resource_documentation": format!("{}/docs", state.public_url),
    }))
}

/// `GET /.well-known/oauth-protected-resource/t/{ns}/mcp` (PRD-mcphost-tenant-resource-metadata
/// requirement 2 / AC1): same shape as [`protected_resource_metadata`], but
/// `resource` is this one tenant's own canonical URI and
/// `authorization_servers` lists issuers this tenant itself registered --
/// never another tenant's, the whole point of a per-tenant document
/// (Non-goals/Goals: "non-enumerable") -- PLUS, when this tenant has an
/// OIDC federation provider registered (`host.oauth.provider_set`,
/// PRD-mcphost-federated-end-user-login), this host's own hosted-AS issuer
/// (AC11: a federated tenant's end users authenticate through mcphost's own
/// authorization-code round trip, not the upstream OIDC provider directly,
/// so a client needs to discover mcphost itself as an authorization server
/// here -- the same value [`protected_resource_metadata`] always lists for
/// the root document). Hosted AS listed first, deduplicated against an own
/// issuer that happens to equal it. A tenant with no provider registered
/// keeps exactly today's behaviour: only its own issuers.
pub async fn protected_resource_metadata_for_tenant(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_oauth_issuers_by_tenant(tenant.id).await?;
    let mut issuers: Vec<String> = rows.into_iter().map(|row| row.issuer).collect();
    if state.db.find_oauth_provider_by_tenant(tenant.id).await?.is_some() {
        let own = state.public_url.trim_end_matches('/').to_string();
        if !issuers.iter().any(|i| i == &own) {
            issuers.insert(0, own);
        }
    }
    // PRD-mcphost-tool-scopes-and-consent requirement 3 (AC7): `mcp`
    // alone for a tenant that has never called `host.oauth.scope_set`
    // (`tenantprm_ac01`'s own pre-existing "empty catalog" pin, unchanged
    // by this PRD) -- once a catalog exists, `mcp`, `read`, `write` lead
    // (in that fixed order -- the built-ins are always valid on a tool's
    // own `scopes` once a tenant has opted into scoping at all,
    // requirement 1), then this tenant's own catalog names beyond those
    // three, sorted.
    let catalog = state.db.list_oauth_scopes(tenant.id).await?;
    let scopes_supported: Vec<String> = if catalog.is_empty() {
        vec!["mcp".to_string()]
    } else {
        let mut scopes: Vec<String> =
            std::iter::once("mcp".to_string()).chain(BUILTIN_SCOPES.iter().map(|s| s.to_string())).collect();
        let mut extra: Vec<String> = catalog
            .into_iter()
            .map(|s| s.name)
            .filter(|name| !scopes.contains(name))
            .collect();
        extra.sort();
        extra.dedup();
        scopes.extend(extra);
        scopes
    };
    Ok(json!({
        "resource": canonical_resource_uri(&state.public_url, &tenant.namespace),
        "authorization_servers": issuers,
        "bearer_methods_supported": ["header"],
        "scopes_supported": scopes_supported,
        "resource_documentation": format!("{}/docs", state.public_url),
    }))
}
