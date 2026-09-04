//! Shared server state and small pure helpers (name/limit validation, time
//! window parsing) used by both the control plane and the admin tools.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::db::Db;
use crate::errors::AppError;
use crate::kinds::KindRegistry;
use crate::registry::RegistryConfig;
use crate::secrets::SecretBox;

pub const MAX_TOOLS_PER_TENANT: i64 = 50;
pub const MAX_SPEC_BYTES: usize = 64 * 1024;
pub const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_CALL_RESULT_BYTES: usize = 1024 * 1024;
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// PRD-mcphost-signup-rate-configurable requirement 1: the fallback used
/// when `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` is absent or unparseable.
/// Also the crate's default when nothing overrides it.
pub const SIGNUP_RATE_LIMIT_PER_HOUR: i64 = 5;
pub const SIGNUP_RATE_LIMIT_WINDOW_SECS: i64 = 3600;
pub const TOOLS_LIST_TTL_GRACE_SECS: u64 = 60;
pub const TOOLS_LIST_TTL_MS_STEADY: u64 = 30_000;
/// PRD-mcphost-code-tools-warm-pool requirement 3 / AC7: `host.tool_run` is
/// rate-limited per tenant independent of any kind's own metering, since it
/// deliberately writes no `calls` row for the usual per-tenant limits to
/// gate on.
pub const TOOL_RUN_RATE_LIMIT_PER_MINUTE: usize = 30;
pub const TOOL_RUN_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// A per-tenant sliding-window call counter for `host.tool_run`
/// (requirement 3 / AC7). Kept in `handler.rs`'s territory (cross-kind,
/// control-plane-level) rather than inside `kinds::python`'s own
/// `CpuBudget`-style limiter, since `host.tool_run` is a single RPC that
/// dispatches to whichever kind the named tool happens to be, not a
/// python-specific concept.
#[derive(Clone, Default)]
pub struct ToolRunLimiter {
    windows: Arc<Mutex<HashMap<i64, VecDeque<Instant>>>>,
}

impl ToolRunLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` if `tenant_id` has room for one more call in the current
    /// window (and records this call if so); `false` (and records nothing)
    /// once the 31st call in 60s arrives.
    pub fn allow(&self, tenant_id: i64) -> bool {
        let now = Instant::now();
        let mut guard = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        let window = guard.entry(tenant_id).or_default();
        while let Some(&oldest) = window.front() {
            if now.duration_since(oldest) >= TOOL_RUN_RATE_LIMIT_WINDOW {
                window.pop_front();
            } else {
                break;
            }
        }
        if window.len() >= TOOL_RUN_RATE_LIMIT_PER_MINUTE {
            return false;
        }
        window.push_back(now);
        true
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub kinds: KindRegistry,
    pub secrets: SecretBox,
    pub admin_key: Option<String>,
    pub public_url: String,
    /// The `Kind::call` deadline (PRD requirement 11: 30s). A field rather
    /// than only the [`CALL_TIMEOUT`] constant so integration tests can
    /// shrink it (AC15) without a real 30-second wait.
    pub call_timeout: Duration,
    /// `Some` only when `--registry-url` / `$MCPHOST_REGISTRY_URL` enabled
    /// the P1 registry-publish feature (requirement 15 / AC19); `None`
    /// makes `host.registry_publish` refuse with a distinct error.
    pub registry: Option<RegistryConfig>,
    /// Shared outbound client `host.registry_publish` POSTs the
    /// `server.json` document with; one client per process, per the usual
    /// `reqwest` connection-pooling advice.
    pub http_client: reqwest::Client,
    /// PRD-mcphost-code-tools requirement 5: the sandbox isolation
    /// mechanism the `python` kind is using (`"bwrap"`, `"unshare+setpriv"`,
    /// or `"none"`), surfaced on `/healthz`. `None` when the `python` kind
    /// isn't registered (e.g. some future minimal deployment).
    pub sandbox_mechanism: Option<&'static str>,
    /// PRD-mcphost-code-tools-warm-pool requirement 3 / AC7: `host.tool_run`'s
    /// own 30-per-minute-per-tenant rate limit.
    pub tool_run_limiter: ToolRunLimiter,
    /// PRD-mcphost-signup-rate-configurable requirement 1: how many
    /// `signup` calls a single source IP may make per
    /// [`SIGNUP_RATE_LIMIT_WINDOW_SECS`] window. Read once at startup from
    /// `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` (see
    /// [`signup_rate_limit_per_hour_from_env`]); a field rather than only
    /// the [`SIGNUP_RATE_LIMIT_PER_HOUR`] constant so a single-IP measure
    /// run can raise the cap without a rebuild.
    pub signup_rate_limit_per_hour: i64,
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `^[a-z][a-z0-9_]{1,40}$` — a lowercase-leading identifier, 2-41 chars.
pub fn validate_tool_name(name: &str) -> Result<(), AppError> {
    let bytes = name.as_bytes();
    let len_ok = (2..=41).contains(&bytes.len());
    let first_ok = bytes.first().is_some_and(|b| b.is_ascii_lowercase());
    let rest_ok = bytes[1.min(bytes.len())..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_');
    if len_ok && first_ok && rest_ok {
        Ok(())
    } else {
        Err(AppError::InvalidToolName(name.to_string()))
    }
}

/// Parse a window like `"24h"`, `"30m"`, `"45s"`, `"2d"` into seconds.
/// Unparseable input defaults to 24h (the PRD's only documented value).
pub fn parse_window_secs(window: &str) -> i64 {
    let window = window.trim();
    let (num_part, unit) = window.split_at(window.len().saturating_sub(1));
    let n: i64 = num_part.parse().unwrap_or(24);
    match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => 24 * 3600,
    }
}

/// Parses the signup rate limit override from a raw string (`None` when
/// `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` is unset), falling back to
/// [`SIGNUP_RATE_LIMIT_PER_HOUR`] when absent or unparseable
/// (PRD-mcphost-signup-rate-configurable requirement 1 / AC3). Pure
/// function, deliberately separated from the `std::env::var` read in
/// [`signup_rate_limit_per_hour_from_env`], so it's testable without
/// mutating process environment (racy across the parallel test threads
/// `cargo test` runs within one binary).
pub fn parse_signup_rate_limit_per_hour(raw: Option<&str>) -> i64 {
    match raw {
        None => SIGNUP_RATE_LIMIT_PER_HOUR,
        Some(raw) => raw.parse::<i64>().unwrap_or_else(|_| {
            tracing::warn!(
                raw,
                default = SIGNUP_RATE_LIMIT_PER_HOUR,
                "MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR is not a valid integer; falling back to default"
            );
            SIGNUP_RATE_LIMIT_PER_HOUR
        }),
    }
}

/// Reads `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` once at startup and logs the
/// effective value (PRD-mcphost-signup-rate-configurable requirement 4 /
/// AC4), so a deployed hub's cap is visible in `journalctl` without
/// reading the binary's environment directly.
pub fn signup_rate_limit_per_hour_from_env() -> i64 {
    let raw = std::env::var("MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR").ok();
    let limit = parse_signup_rate_limit_per_hour(raw.as_deref());
    tracing::info!(
        signup_rate_limit_per_hour = limit,
        "signup rate limit configured"
    );
    limit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name_rules() {
        assert!(validate_tool_name("hello").is_ok());
        assert!(validate_tool_name("h").is_err()); // too short
        assert!(validate_tool_name("Hello").is_err()); // uppercase
        assert!(validate_tool_name("1hello").is_err()); // must start lowercase letter
        assert!(validate_tool_name("hello-world").is_err()); // dash not allowed
        assert!(validate_tool_name(&"a".repeat(42)).is_err()); // too long
        assert!(validate_tool_name(&"a".repeat(41)).is_ok());
    }

    #[test]
    fn window_parsing() {
        assert_eq!(parse_window_secs("24h"), 24 * 3600);
        assert_eq!(parse_window_secs("30m"), 30 * 60);
        assert_eq!(parse_window_secs("45s"), 45);
        assert_eq!(parse_window_secs("2d"), 2 * 86400);
    }

    /// PRD-mcphost-signup-rate-configurable requirement 1: absent falls
    /// back to the default.
    #[test]
    fn signup_rate_limit_absent_falls_back_to_default() {
        assert_eq!(
            parse_signup_rate_limit_per_hour(None),
            SIGNUP_RATE_LIMIT_PER_HOUR
        );
    }

    /// AC3: a non-integer value falls back to the default (the log call is
    /// exercised, not asserted on here -- this crate has no log-capture
    /// harness; the effective value is what's contractually observable).
    #[test]
    fn signup_rate_limit_unparseable_falls_back_to_default() {
        assert_eq!(
            parse_signup_rate_limit_per_hour(Some("not-a-number")),
            SIGNUP_RATE_LIMIT_PER_HOUR
        );
    }

    /// AC2: a valid override is honored verbatim.
    #[test]
    fn signup_rate_limit_valid_override_is_honored() {
        assert_eq!(parse_signup_rate_limit_per_hour(Some("100")), 100);
    }

    /// PRD-mcphost-code-tools-warm-pool AC7: the 30th call in a tenant's
    /// window succeeds, the 31st does not; a different tenant's own window
    /// is unaffected.
    #[test]
    fn tool_run_limiter_allows_thirty_then_blocks() {
        let limiter = ToolRunLimiter::new();
        for i in 0..TOOL_RUN_RATE_LIMIT_PER_MINUTE {
            assert!(limiter.allow(1), "call {i} within the limit must be allowed");
        }
        assert!(
            !limiter.allow(1),
            "the 31st call in the window must be refused"
        );
        assert!(
            limiter.allow(2),
            "a different tenant's own window must be independent"
        );
    }
}
