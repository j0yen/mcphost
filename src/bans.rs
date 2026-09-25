//! PRD-mcphost-abuse-guard-ban-list: the ban list itself -- an in-memory
//! cache of every currently-active `bans` row (requirement 6 / AC10, so
//! enforcement costs one hash-map lookup per request, not a query), the
//! shared `enforce` check every enforcement point in the crate calls, and
//! the minute tick that raises the two auto-ban rules (requirement 4) and
//! sweeps long-expired rows (requirement 5).
//!
//! Requirement 4's "if the alerting registry is present, raise `ban.applied`"
//! has no code here: this crate has no alerting registry today, so that
//! condition is vacuously false on every build -- there is nothing to wire
//! up until one exists.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::db::Db;
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// requirement 4: both auto-ban rules look at the trailing 10 minutes.
pub const AUTO_BAN_WINDOW_SECS: i64 = 600;
/// requirement 4: both auto-ban rules mint a 1 hour ban.
pub const AUTO_BAN_TTL_SECS: i64 = 3600;
/// requirement 5: a ban stays in `bans` (for `admin.ban.list`'s history)
/// for 7 days past its own expiry before the sweep removes the row.
pub const EXPIRED_SWEEP_AGE_SECS: i64 = 7 * 86_400;
/// requirement 4(a)'s default, absent `$MCPHOST_BAN_DENIALS_THRESHOLD`.
pub const BAN_DENIALS_THRESHOLD_DEFAULT: i64 = 25;
/// requirement 4(b)'s default, absent `$MCPHOST_BAN_CLAIM_RATE_THRESHOLD`.
pub const BAN_CLAIM_RATE_THRESHOLD_DEFAULT: i64 = 30;

/// requirement 2: `subject_kind: "key"`'s at-rest (and lookup) form is
/// always the sha256 hex of the raw tenant key -- same hash-at-rest
/// convention `tenants.key_hash` already uses, so `admin.ban.add`'s operator
/// input (the raw key) and `resolve_auth`'s already-hashed `tenant.key_hash`
/// meet in the same form without this crate ever storing a credential in
/// the clear. `addr`/`email_domain` are literal, unmodified.
pub fn normalize_subject(subject_kind: &str, raw: &str) -> String {
    if subject_kind == "key" {
        crate::auth::hash_key(raw)
    } else {
        raw.to_string()
    }
}

struct Entry {
    id: i64,
    reason: String,
    public: bool,
    expires_at: Option<i64>,
}

/// `(subject_kind, subject)` -> its active [`Entry`] -- factored into a
/// named alias so [`BanCache`]'s field declaration doesn't trip
/// clippy::type_complexity.
type BanMap = HashMap<(String, String), Entry>;

/// [`BanCache::check`]'s hit shape: `reason` is already the public/private
/// decision (requirement 2 -- `None` unless the ban was made `public: true`),
/// so no caller needs to re-check `public` itself.
pub struct BanHit {
    pub id: i64,
    pub reason: Option<String>,
    pub expires_at: Option<i64>,
}

/// requirement 6 / AC10: an in-memory mirror of every currently-active
/// `bans` row, keyed by `(subject_kind, normalized subject)`, refreshed
/// synchronously right after every `admin.ban.add`/`remove` and
/// [`tick_once`], plus every 30s from [`spawn_cache_refresh`] -- so
/// enforcement is one `HashMap` lookup, never a query, regardless of how
/// many bans are active (non-functional requirement: <50µs/request,
/// AC10: p99 <1ms against 10k entries).
#[derive(Clone, Default)]
pub struct BanCache {
    inner: Arc<RwLock<BanMap>>,
}

impl BanCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// `None` when `(subject_kind, subject)` isn't banned, or its ban has
    /// expired -- checked live against [`now_unix`] here (not just at
    /// refresh time), so AC6's "no restart, and no wait for the next 30s
    /// refresh either" holds: a ban whose `expires_at` just passed reads as
    /// not-banned on the very next call even from an unrefreshed cache.
    pub fn check(&self, subject_kind: &str, subject: &str) -> Option<BanHit> {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let entry = guard.get(&(subject_kind.to_string(), subject.to_string()))?;
        if let Some(expires_at) = entry.expires_at
            && expires_at <= now_unix()
        {
            return None;
        }
        Some(BanHit {
            id: entry.id,
            reason: entry.public.then(|| entry.reason.clone()),
            expires_at: entry.expires_at,
        })
    }

    /// Reloads the whole cache from `db.list_active_bans()`. Called after
    /// every write (`admin.ban.add`/`remove`, [`tick_once`]'s auto-bans and
    /// sweep) and periodically by [`spawn_cache_refresh`].
    pub async fn refresh(&self, db: &Db) -> Result<(), AppError> {
        let rows = db.list_active_bans().await?;
        let mut map = HashMap::with_capacity(rows.len());
        for row in rows {
            map.insert(
                (row.subject_kind, row.subject),
                Entry {
                    id: row.id,
                    reason: row.reason,
                    public: row.public,
                    expires_at: row.expires_at,
                },
            );
        }
        *self.inner.write().unwrap_or_else(|e| e.into_inner()) = map;
        Ok(())
    }
}

/// requirement 2: the one check every enforcement point (`signup`'s addr,
/// an authenticated tool call's key, `/claim/*`'s addr/email_domain,
/// `/hooks/*`'s owning-tenant key) calls. `Ok(())` for an unbanned subject
/// (the overwhelmingly common case: one cache lookup, nothing else).
/// `Err(AppError::banned(..))` for a banned one -- also records the hit
/// (AC2's `hits` increments; best-effort, a recording failure must never
/// turn an already-refused call into a successful one).
pub async fn enforce(state: &AppState, subject_kind: &str, subject: &str) -> Result<(), AppError> {
    let Some(hit) = state.bans.check(subject_kind, subject) else {
        return Ok(());
    };
    if let Err(e) = state.db.record_ban_hit(hit.id).await {
        tracing::warn!(error = %e, ban_id = hit.id, "failed to record ban hit");
    }
    Err(AppError::banned(hit.expires_at, hit.reason))
}

/// requirement 4/5: one minute-tick cycle -- raise both auto-ban rules
/// (de-duped against an already-active ban for the same subject, so a
/// subject that stays over threshold across several ticks gets one ban
/// row, not one per tick), sweep rows expired more than 7 days, then
/// refresh the cache. Exposed directly (not just via [`spawn_tick`]) so a
/// test can drive one deterministic cycle instead of waiting on the real
/// 60s cadence -- same convention as `triggers::tick_once`.
pub async fn tick_once(state: &AppState) -> Result<(), AppError> {
    let now = now_unix();
    let since = now - AUTO_BAN_WINDOW_SECS;

    let tenant_ids = state
        .db
        .tenants_over_denial_threshold(since, state.ban_denials_threshold)
        .await?;
    for tenant_id in tenant_ids {
        let Some(tenant) = state.db.find_tenant_by_id(tenant_id).await? else {
            continue;
        };
        if state
            .db
            .has_active_ban("key".to_string(), tenant.key_hash.clone())
            .await?
        {
            continue;
        }
        state
            .db
            .insert_ban(
                "key".to_string(),
                tenant.key_hash.clone(),
                format!(
                    "auto: network_denials >= {} in {}m",
                    state.ban_denials_threshold,
                    AUTO_BAN_WINDOW_SECS / 60
                ),
                false,
                "auto".to_string(),
                Some(now + AUTO_BAN_TTL_SECS),
                true,
            )
            .await?;
        tracing::warn!(
            tenant = %tenant.namespace,
            threshold = state.ban_denials_threshold,
            "auto-banned tenant key: network_denials threshold exceeded"
        );
    }

    let addrs = state
        .db
        .addrs_over_claim_rate_threshold(since, state.ban_claim_rate_threshold)
        .await?;
    for addr in addrs {
        if state.db.has_active_ban("addr".to_string(), addr.clone()).await? {
            continue;
        }
        state
            .db
            .insert_ban(
                "addr".to_string(),
                addr.clone(),
                format!(
                    "auto: claim_rate_events >= {} in {}m",
                    state.ban_claim_rate_threshold,
                    AUTO_BAN_WINDOW_SECS / 60
                ),
                false,
                "auto".to_string(),
                Some(now + AUTO_BAN_TTL_SECS),
                true,
            )
            .await?;
        tracing::warn!(
            addr = %addr,
            threshold = state.ban_claim_rate_threshold,
            "auto-banned address: claim_rate_events threshold exceeded"
        );
    }

    state.db.sweep_expired_bans(now - EXPIRED_SWEEP_AGE_SECS).await?;
    state.bans.refresh(&state.db).await?;
    Ok(())
}

/// requirement 6: the 30s periodic cache refresh, started once alongside
/// the other background tasks in `main.rs` -- same spawn/sleep-loop shape
/// as `channels::spawn_channel_retention`.
pub fn spawn_cache_refresh(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            if let Err(e) = state.bans.refresh(&state.db).await {
                tracing::warn!(error = %e, "ban cache periodic refresh failed");
            }
        }
    })
}

/// requirement 4/5: the minute tick, started once alongside
/// [`spawn_cache_refresh`].
pub fn spawn_tick(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            if let Err(e) = tick_once(&state).await {
                tracing::warn!(error = %e, "ban tick failed");
            }
        }
    })
}

/// Reads `$MCPHOST_BAN_DENIALS_THRESHOLD`, falling back to
/// [`BAN_DENIALS_THRESHOLD_DEFAULT`] when absent or unparseable -- same
/// pure-parse-then-fallback shape `state::parse_signup_rate_limit_per_hour`
/// already establishes.
pub fn parse_ban_denials_threshold(raw: Option<&str>) -> i64 {
    match raw {
        None => BAN_DENIALS_THRESHOLD_DEFAULT,
        Some(raw) => raw.parse::<i64>().unwrap_or_else(|_| {
            tracing::warn!(
                raw,
                default = BAN_DENIALS_THRESHOLD_DEFAULT,
                "MCPHOST_BAN_DENIALS_THRESHOLD is not a valid integer; falling back to default"
            );
            BAN_DENIALS_THRESHOLD_DEFAULT
        }),
    }
}

pub fn ban_denials_threshold_from_env() -> i64 {
    parse_ban_denials_threshold(std::env::var("MCPHOST_BAN_DENIALS_THRESHOLD").ok().as_deref())
}

/// Same shape, for `$MCPHOST_BAN_CLAIM_RATE_THRESHOLD`.
pub fn parse_ban_claim_rate_threshold(raw: Option<&str>) -> i64 {
    match raw {
        None => BAN_CLAIM_RATE_THRESHOLD_DEFAULT,
        Some(raw) => raw.parse::<i64>().unwrap_or_else(|_| {
            tracing::warn!(
                raw,
                default = BAN_CLAIM_RATE_THRESHOLD_DEFAULT,
                "MCPHOST_BAN_CLAIM_RATE_THRESHOLD is not a valid integer; falling back to default"
            );
            BAN_CLAIM_RATE_THRESHOLD_DEFAULT
        }),
    }
}

pub fn ban_claim_rate_threshold_from_env() -> i64 {
    parse_ban_claim_rate_threshold(std::env::var("MCPHOST_BAN_CLAIM_RATE_THRESHOLD").ok().as_deref())
}

/// requirement 3: parses `ttl` (`"30m"`, `"24h"`, `"7d"` -- a whole number
/// plus one of `m`/`h`/`d`) into seconds. `None` for anything else
/// (`admin.ban.add` reports that as a validation error naming the field).
pub fn parse_ban_ttl_secs(ttl: &str) -> Option<i64> {
    let ttl = ttl.trim();
    if ttl.is_empty() {
        return None;
    }
    let (num_part, unit) = ttl.split_at(ttl.len() - 1);
    let n: i64 = num_part.parse().ok()?;
    if n <= 0 {
        return None;
    }
    match unit {
        "m" => Some(n * 60),
        "h" => Some(n * 3600),
        "d" => Some(n * 86_400),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_subject_hashes_key_only() {
        let hashed = normalize_subject("key", "raw-tenant-key");
        assert_ne!(hashed, "raw-tenant-key");
        assert_eq!(hashed, crate::auth::hash_key("raw-tenant-key"));
        assert_eq!(normalize_subject("addr", "203.0.113.9"), "203.0.113.9");
        assert_eq!(normalize_subject("email_domain", "example.com"), "example.com");
    }

    #[test]
    fn ttl_parsing() {
        assert_eq!(parse_ban_ttl_secs("30m"), Some(30 * 60));
        assert_eq!(parse_ban_ttl_secs("24h"), Some(24 * 3600));
        assert_eq!(parse_ban_ttl_secs("7d"), Some(7 * 86_400));
        assert_eq!(parse_ban_ttl_secs(""), None);
        assert_eq!(parse_ban_ttl_secs("30"), None);
        assert_eq!(parse_ban_ttl_secs("0m"), None);
        assert_eq!(parse_ban_ttl_secs("-1h"), None);
        assert_eq!(parse_ban_ttl_secs("30x"), None);
    }

    #[test]
    fn cache_check_respects_live_expiry() {
        let cache = BanCache::new();
        {
            let mut guard = cache.inner.write().unwrap();
            guard.insert(
                ("addr".to_string(), "203.0.113.9".to_string()),
                Entry {
                    id: 1,
                    reason: "flood".to_string(),
                    public: false,
                    expires_at: Some(now_unix() - 1),
                },
            );
        }
        assert!(cache.check("addr", "203.0.113.9").is_none());
    }

    #[test]
    fn cache_check_reports_reason_only_when_public() {
        let cache = BanCache::new();
        {
            let mut guard = cache.inner.write().unwrap();
            guard.insert(
                ("addr".to_string(), "203.0.113.9".to_string()),
                Entry {
                    id: 1,
                    reason: "flood".to_string(),
                    public: false,
                    expires_at: None,
                },
            );
            guard.insert(
                ("addr".to_string(), "203.0.113.10".to_string()),
                Entry {
                    id: 2,
                    reason: "flood".to_string(),
                    public: true,
                    expires_at: None,
                },
            );
        }
        assert_eq!(cache.check("addr", "203.0.113.9").unwrap().reason, None);
        assert_eq!(
            cache.check("addr", "203.0.113.10").unwrap().reason,
            Some("flood".to_string())
        );
    }
}
