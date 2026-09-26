//! PRD-mcphost-end-user-identity: the end-user identity a call carries
//! (from an OAuth bearer's `sub`/`iss`, or a tenant-signed `end_user_assertion`
//! for key-based callers), `host.enduser.*`, and `tenant_state.rs`'s shared
//! `end_user` argument resolution (requirement 5).

use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// requirement 1/2: how an [`EndUser`] was verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndUserMethod {
    Oauth,
    Assertion,
}

impl EndUserMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            EndUserMethod::Oauth => "oauth",
            EndUserMethod::Assertion => "assertion",
        }
    }
}

/// `Caller.end_user` (requirement 1): a verified human/end-user identity
/// this call carries, never fabricated -- an unverified claim never
/// produces one of these (Goals: "unverified claim is absent, not
/// present").
#[derive(Debug, Clone, PartialEq)]
pub struct EndUser {
    pub subject: String,
    /// Only ever `Some` for [`EndUserMethod::Oauth`] -- an assertion's
    /// `sub` carries no issuer of its own (requirement 2 names no `iss`
    /// claim for it).
    pub issuer: Option<String>,
    pub method: EndUserMethod,
    pub verified_at: i64,
}

impl EndUser {
    pub fn whoami_json(&self) -> Value {
        json!({
            "subject": self.subject,
            "issuer": self.issuer,
            "method": self.method.as_str(),
            "verified_at": self.verified_at,
        })
    }
}

fn assertion_invalid(reason: &str) -> AppError {
    AppError::Structured {
        code: "end_user_assertion_invalid",
        message: format!("end_user_assertion is invalid: {reason}"),
        data: json!({"reason": reason}),
    }
}

/// requirement 2: `end_user_assertion`'s `exp <= iat + 3600`.
const ASSERTION_MAX_TTL_SECS: i64 = 3600;

/// requirement 2: the reserved per-tenant secret name the assertion HS256
/// key is stored under (`secrets.rs`'s existing AES-256-GCM encryption at
/// rest, same table `host.secret_set` writes to) -- `host.secret_list`
/// shows it exists (it's just another name in that list) without ever
/// revealing its value (that endpoint never returns values, for any name).
/// A tenant cannot `host.secret_set` this name directly (see
/// `control::secret_set`'s guard) so only `assertion_secret_rotate` below
/// ever changes it.
pub const ASSERTION_SECRET_NAME: &str = "__mcphost_end_user_assertion__";

#[derive(Serialize, Deserialize)]
struct AssertionClaims {
    sub: String,
    iat: i64,
    exp: i64,
}

/// requirement 2 (AC2/AC3): verifies a compact HS256 JWS `end_user_assertion`
/// against this tenant's current assertion secret. Every failure -- no
/// secret set yet, wrong signature, malformed claims, `exp` in the past,
/// or `exp` further than [`ASSERTION_MAX_TTL_SECS`] past `iat` -- returns
/// the same `end_user_assertion_invalid` code (AC3), never falling back to
/// "no end user" (requirement 2: "never fall back to 'no end user'" --
/// this function's `Err` is the caller's cue to refuse the whole call, not
/// treat it as anonymous).
pub async fn verify_assertion(
    state: &AppState,
    tenant: &Tenant,
    assertion: &str,
) -> Result<EndUser, AppError> {
    let Some((ct, nonce)) = state.db.get_secret(tenant.id, ASSERTION_SECRET_NAME.to_string()).await? else {
        return Err(assertion_invalid("no assertion secret is set for this tenant"));
    };
    let secret = state.secrets.decrypt(&ct, &nonce)?;

    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    validation.required_spec_claims.clear();
    let decoding_key = DecodingKey::from_secret(secret.as_bytes());
    let data = jsonwebtoken::decode::<AssertionClaims>(assertion, &decoding_key, &validation)
        .map_err(|_| assertion_invalid("signature or shape is invalid"))?;
    let claims = data.claims;

    if claims.sub.is_empty() {
        return Err(assertion_invalid("sub must be non-empty"));
    }
    if claims.exp > claims.iat + ASSERTION_MAX_TTL_SECS {
        return Err(assertion_invalid("exp exceeds iat + 3600"));
    }
    let now = now_unix();
    if now > claims.exp {
        return Err(assertion_invalid("exp is in the past"));
    }

    Ok(EndUser {
        subject: claims.sub,
        issuer: None,
        method: EndUserMethod::Assertion,
        verified_at: now,
    })
}

/// `host.enduser.assertion_secret_rotate` (AC11): generates a fresh HS256
/// secret, stores it (encrypted, same mechanism `host.secret_set` uses)
/// under [`ASSERTION_SECRET_NAME`], and returns it once -- the only time
/// its value is ever visible to the tenant. Assertions signed with any
/// prior secret stop verifying the instant this lands (no grace period:
/// requirement 2 names no overlap window).
pub async fn assertion_secret_rotate(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let secret = crate::auth::generate_key();
    let (ct, nonce) = state.secrets.encrypt(&secret)?;
    state
        .db
        .upsert_secret(tenant.id, ASSERTION_SECRET_NAME.to_string(), ct, nonce)
        .await?;
    Ok(json!({"secret": secret, "rotated": true}))
}

/// requirement 7 (AC10): `host.enduser.whoami`.
pub fn whoami(end_user: Option<&EndUser>) -> Value {
    match end_user {
        Some(eu) => eu.whoami_json(),
        None => Value::Null,
    }
}

/// requirement 5: resolves the `end_user` argument (`"self" | "<subject>" |
/// null`) shared by every `host.state.*` storage op this PRD scopes.
/// Returns `(end_user_norm, impersonated)` -- `end_user_norm` is `""` for
/// tenant-wide (matches migration 0039's storage sentinel) or the resolved
/// subject; `impersonated` is `true` only for the explicit-subject branch
/// (requirement 5: "recorded on the call row as impersonated" -- this
/// crate surfaces it on the op's own JSON result instead of a `calls` row,
/// since `host.state.*`/`mcphost.state` calls write no `calls` row of
/// their own to mark).
pub fn resolve_end_user(args: &Value, call_end_user: Option<&EndUser>) -> Result<(String, bool), AppError> {
    match args.get("end_user") {
        None | Some(Value::Null) => Ok((String::new(), false)),
        Some(Value::String(s)) if s == "self" => {
            let eu = call_end_user.ok_or_else(|| AppError::Structured {
                code: "end_user_required",
                message: "end_user: \"self\" requires a verified end-user identity on this call"
                    .to_string(),
                data: json!({}),
            })?;
            Ok((eu.subject.clone(), false))
        }
        Some(Value::String(s)) => {
            if call_end_user.is_some() {
                return Err(AppError::Structured {
                    code: "end_user_explicit_forbidden",
                    message: "an explicit end_user subject is only allowed when the call carries \
                        no end-user identity of its own"
                        .to_string(),
                    data: json!({}),
                });
            }
            Ok((s.clone(), true))
        }
        Some(_) => Err(AppError::InvalidArgs(
            "end_user must be a string (\"self\" or a subject) or null".to_string(),
        )),
    }
}

/// requirement 6 (AC9): rejects a `host.state.set`/`insert` write from a
/// new (not already active in the trailing 30 days) end-user subject once
/// the tenant's plan `end_users_max` distinct active subjects is already
/// reached. `end_user_norm == ""` (tenant-wide) is always allowed --
/// there's no "subject" to count. Callers touch
/// [`crate::db::Db::touch_end_user_activity`] themselves after a write
/// this allowed actually lands.
pub async fn check_end_user_quota(state: &AppState, tenant: &Tenant, end_user_norm: &str) -> Result<(), AppError> {
    if end_user_norm.is_empty() {
        return Ok(());
    }
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let since = now_unix() - 30 * 86_400;
    let already_active = state
        .db
        .end_user_active_since(tenant.id, end_user_norm.to_string(), since)
        .await?;
    if already_active {
        return Ok(());
    }
    let used = state.db.count_distinct_end_users_since(tenant.id, since).await?;
    if used >= plan.end_users_max {
        return Err(AppError::Structured {
            code: "quota_end_users",
            message: format!(
                "distinct end users writing state in the trailing 30 days: {used}, plan maximum {}",
                plan.end_users_max
            ),
            data: json!({"quota": "end_users_max", "limit": plan.end_users_max, "used": used}),
        });
    }
    Ok(())
}

/// Not directly exercised by any test here -- kept only so
/// `EncodingKey`/`Header` (used by `tests/support` to mint assertions the
/// same way a real tenant client would) stay obviously reachable from this
/// module's own public surface for anyone reading it top to bottom.
#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header};

    #[test]
    fn resolve_end_user_defaults_tenant_wide() {
        assert_eq!(resolve_end_user(&json!({}), None).unwrap(), (String::new(), false));
        assert_eq!(
            resolve_end_user(&json!({"end_user": null}), None).unwrap(),
            (String::new(), false)
        );
    }

    #[test]
    fn resolve_end_user_self_requires_identity() {
        let err = resolve_end_user(&json!({"end_user": "self"}), None).unwrap_err();
        assert_eq!(err.code(), "end_user_required");
    }

    #[test]
    fn resolve_end_user_self_resolves_subject() {
        let eu = EndUser {
            subject: "u1".to_string(),
            issuer: None,
            method: EndUserMethod::Oauth,
            verified_at: 1,
        };
        let (norm, impersonated) = resolve_end_user(&json!({"end_user": "self"}), Some(&eu)).unwrap();
        assert_eq!(norm, "u1");
        assert!(!impersonated);
    }

    #[test]
    fn resolve_end_user_explicit_forbidden_with_identity() {
        let eu = EndUser {
            subject: "u1".to_string(),
            issuer: None,
            method: EndUserMethod::Oauth,
            verified_at: 1,
        };
        let err = resolve_end_user(&json!({"end_user": "u9"}), Some(&eu)).unwrap_err();
        assert_eq!(err.code(), "end_user_explicit_forbidden");
    }

    #[test]
    fn resolve_end_user_explicit_allowed_without_identity() {
        let (norm, impersonated) = resolve_end_user(&json!({"end_user": "u9"}), None).unwrap();
        assert_eq!(norm, "u9");
        assert!(impersonated);
    }

    /// Exercises the sign path a real caller's client would use, so
    /// `EncodingKey`/`Header` earn their import here too (mirrors
    /// `oauth.rs`'s own unit-test convention of a minimal round trip
    /// alongside the integration suite's fuller coverage).
    #[tokio::test]
    async fn verify_assertion_round_trips_a_freshly_signed_token() {
        let dir = std::env::temp_dir().join(format!(
            "mcphost-enduser-test-{}-{}",
            std::process::id(),
            now_unix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Db::open(&dir).unwrap();
        db.migrate().await.unwrap();
        let secrets = crate::secrets::SecretBox::from_passphrase("test-passphrase-value");
        let tenant = db
            .create_tenant("t".to_string(), "t1".to_string(), "hash1".to_string(), None)
            .await
            .unwrap();

        let assertion_value = "unit-test-assertion-value";
        let (ct, nonce) = secrets.encrypt(assertion_value).unwrap();
        db.upsert_secret(tenant.id, ASSERTION_SECRET_NAME.to_string(), ct, nonce)
            .await
            .unwrap();

        let now = now_unix();
        let claims = AssertionClaims { sub: "u1".to_string(), iat: now, exp: now + 60 };
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(assertion_value.as_bytes()),
        )
        .unwrap();

        let state = crate::state::AppState {
            db,
            kinds: crate::kinds::KindRegistry::with_builtin(),
            secrets,
            admin_key: None,
            public_url: "http://127.0.0.1:0".to_string(),
            call_timeout: crate::state::CALL_TIMEOUT,
            registry: None,
            http_client: reqwest::Client::new(),
            sandbox_mechanism: None,
            wasm_runtime_version: None,
            tool_run_limiter: crate::state::ToolRunLimiter::new(),
            signup_rate_limit_per_hour: crate::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            plans: crate::plans::PlanCatalog::default_catalog(),
            billing_config: crate::billing::BillingConfig::default(),
            billing_client: std::sync::Arc::new(crate::billing::FakeBillingClient::new(now_unix())),
            checkout_sessions: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            accepted_usage_cache: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            runs: crate::runs::RunsRegistry::new(),
            scheduler: crate::triggers::SchedulerStatus::new(),
            event_counters: crate::hooks::EventCounters::new(),
            event_rate_limiter: crate::hooks::EventRateLimiter::new(),
            deprecations: std::sync::Arc::new(Vec::new()),
            disk_guard: crate::retention::DiskGuard::from_env(),
            compat_token: None,
            signup_pause: crate::state::SignupPause::from_env(&dir),
            claim_token_ttl_secs: crate::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            claim_rate_limit_per_hour: crate::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            email_config: crate::email::EmailConfig::default(),
            email_client: std::sync::Arc::new(crate::email::FakeEmailClient::new()),
            bans: crate::bans::BanCache::new(),
            ban_denials_threshold: crate::bans::BAN_DENIALS_THRESHOLD_DEFAULT,
            ban_claim_rate_threshold: crate::bans::BAN_CLAIM_RATE_THRESHOLD_DEFAULT,
            oauth: crate::oauth::JwksCache::new(),
            oauth_allowed_algs: crate::oauth::parse_allowed_algs(None),
            oauth_jwks_ttl_secs: crate::oauth::DEFAULT_JWKS_TTL_SECS,
            alerts: crate::alerts::AlertRegistry::new(),
            contention_tracker: crate::alerts::ContentionTracker::new(),
            alert_config: crate::alerts::AlertConfig::default(),
            alert_quota_trips: crate::alerts::QuotaTripTracker::new(),
            status_probe_override: crate::statusfeed::ProbeOverrides::new(),
            fleet_ips: crate::state::FleetIps::empty(),
        };

        let eu = verify_assertion(&state, &tenant, &token).await.expect("valid assertion");
        assert_eq!(eu.subject, "u1");
        assert_eq!(eu.method, EndUserMethod::Assertion);

        std::fs::remove_dir_all(&dir).ok();
    }
}
