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
/// PRD-mcphost-call-limits-honest requirement 2: the one cap named and
/// enforced identically for both the `python` kind's stdout envelope and
/// the `http` kind's upstream response body (that one already enforced
/// this exact number under `response_too_large`; this is the same limit
/// under one name, not a new number). Overrun on either kind returns a
/// structured error naming `limit_bytes`/`actual_bytes` -- `python`'s
/// `tool_output_too_large`, `http`'s existing `response_too_large`.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 1024 * 1024;
/// PRD-mcphost-call-limits-honest requirement 1: this is now the *default*
/// per-call deadline (a spec that declares its own `timeout_s` gets that
/// instead, bounded by the kind's own maximum -- `python`'s
/// [`crate::kinds::python::MAX_TIMEOUT_S`]) -- not the ceiling every call
/// was silently held to before this PRD, regardless of what it declared.
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
    /// PRD-grand-loop-billing requirement 1: the loaded `plans.toml` --
    /// price and quotas per plan. Loaded once at startup
    /// ([`crate::plans::PlanCatalog::load_or_init`]); every quota check
    /// reads a [`crate::plans::Plan`] out of this rather than a constant.
    pub plans: crate::plans::PlanCatalog,
    /// The `MCPHOST_STRIPE_*` settings; `None` fields are the supported
    /// "billing absent" state (goal 4).
    pub billing_config: crate::billing::BillingConfig,
    /// The outbound Checkout-Session client `billing.checkout` calls --
    /// `StripeClient` in production, `FakeBillingClient` in every test
    /// (technical considerations: "Tests never reach the network").
    pub billing_client: std::sync::Arc<dyn crate::billing::BillingClient>,
    /// P1 AC13: open, unexpired Checkout Sessions keyed by
    /// `(tenant_id, plan)`, so a second `billing.checkout` call for the
    /// same pair before expiry returns the cached URL instead of asking
    /// the processor to mint another session. In-memory only -- see
    /// [`crate::billing::OpenCheckoutSession`]. `Arc`-wrapped (like
    /// [`AppState::db`] and every other shared-mutable field here) so
    /// `AppState`'s own `#[derive(Clone)]` stays cheap and every clone
    /// keeps seeing the same cache rather than a forked copy.
    pub checkout_sessions: crate::billing::CheckoutSessionCache,
    /// P1 requirement 60 / AC12: `billing.status`'s cache of Stripe's own
    /// accepted-usage read per tenant, so repeated polling within
    /// [`crate::billing::ACCEPTED_USAGE_CACHE_TTL_SECS`] doesn't turn into
    /// repeated Stripe reads. In-memory only, same lifetime and
    /// `Arc`-sharing rationale as [`AppState::checkout_sessions`].
    pub accepted_usage_cache: crate::billing::AcceptedUsageCache,
    /// PRD-mcphost-runs-and-jobs requirement 4: the executor's live
    /// run-id -> cancel-pid registry, plus (P1 requirement 9's `runs.wait`)
    /// the wake channel a finalized run notifies. In-memory only, same
    /// `Arc`-sharing rationale as [`AppState::checkout_sessions`] -- see
    /// [`crate::runs::RunsRegistry`].
    pub runs: crate::runs::RunsRegistry,
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Days since the civil epoch (1970-01-01) -> `(year, month, day)`, per
/// Howard Hinnant's `civil_from_days` algorithm
/// (http://howardhinnant.github.io/date_algorithms.html). This crate has no
/// chrono/time dependency; PRD-mcphost-sandbox-ready's `sandbox_checked_at`
/// (an RFC 3339 UTC timestamp) doesn't need one either, so this is the
/// whole calendar conversion this crate requires, self-contained and unit
/// tested below rather than trusted blind.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// A minimal RFC 3339 UTC timestamp (`"YYYY-MM-DDTHH:MM:SSZ"`) from a Unix
/// timestamp. Pure function, separated from [`rfc3339_now`]'s `SystemTime`
/// read for the same reason [`parse_signup_rate_limit_per_hour`] is
/// separated from its own env read: testable without depending on wall
/// clock.
pub fn rfc3339_from_unix(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// PRD-mcphost-sandbox-ready requirement 1/2: `sandbox_checked_at`'s value,
/// computed fresh each call.
pub fn rfc3339_now() -> String {
    rfc3339_from_unix(now_unix())
}

/// Milliseconds since the Unix epoch -- [`new_ulid`]'s own time component
/// needs millisecond resolution, coarser than [`now_unix`]'s seconds.
pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Crockford base32 alphabet -- excludes I, L, O, U to avoid transcription
/// confusion, the same alphabet a real ULID uses.
const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// PRD-mcphost-runs-and-jobs requirement 1: `runs.id (ulid)` -- a
/// lexicographically-sortable, time-prefixed identifier so `ORDER BY id`
/// (the leasing query's own ordering, see `db::Db::lease_next_queued_run`)
/// returns insertion order without a second column. This crate has no
/// `ulid`/`uuid` dependency (the same "no new dependency for a small,
/// self-contained thing" call `rfc3339_from_unix`'s hand-rolled calendar
/// math already made); this hand-rolls the two ULID halves instead: 48 bits
/// of millisecond timestamp (10 Crockford base32 chars) followed by 80 bits
/// of randomness (16 chars) via `rand` (already a dependency), for a
/// 26-character id. Not validated against the ULID spec's exact overflow
/// edge cases (this crate never needs to parse one back, only generate and
/// sort it), but sortable and collision-resistant enough for a per-tenant
/// run ledger.
pub fn new_ulid() -> String {
    use rand::RngCore;
    let ts = now_unix_ms().max(0) as u64;
    let mut rng = rand::thread_rng();
    let mut rand_bytes = [0u8; 10]; // 80 bits
    rng.fill_bytes(&mut rand_bytes);

    let mut out = String::with_capacity(26);
    // 48-bit timestamp -> 10 base32 chars, most significant first.
    for i in (0..10).rev() {
        let shift = i * 5;
        let idx = ((ts >> shift) & 0x1f) as usize;
        out.push(CROCKFORD_ALPHABET[idx] as char);
    }
    // 80 bits of randomness -> 16 base32 chars. Packed 5 bits at a time
    // across the 10-byte buffer.
    let mut acc: u128 = 0;
    for b in rand_bytes {
        acc = (acc << 8) | b as u128;
    }
    for i in (0..16).rev() {
        let shift = i * 5;
        let idx = ((acc >> shift) & 0x1f) as usize;
        out.push(CROCKFORD_ALPHABET[idx] as char);
    }
    out
}

/// The unix timestamp of the most recent UTC midnight at or before
/// `now_unix` -- PRD-grand-loop-billing's `calls_per_day` window start
/// (requirement: "counted from the existing `calls` table" by
/// `started_unix`, UTC day).
pub fn utc_midnight_unix(now_unix: i64) -> i64 {
    now_unix.div_euclid(86_400) * 86_400
}

/// The next UTC midnight after `now_unix` -- `billing.status`'s
/// `resets_at` (AC3/AC14).
pub fn next_utc_midnight_unix(now_unix: i64) -> i64 {
    utc_midnight_unix(now_unix) + 86_400
}

/// The inverse of [`civil_from_days`] (same Howard Hinnant algorithm,
/// `days_from_civil`): a UTC calendar date -> days since the civil epoch
/// (1970-01-01). Only [`utc_month_start_unix`] needs this direction of the
/// conversion.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as u64 + 2) / 5 + d as u64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64 - 719_468
}

/// The unix timestamp of `now_unix`'s UTC month, day 1, 00:00:00 --
/// PRD-mcphost-metered-overage's "current month" boundary for
/// `admin.meter_status`'s per-tenant emitted counts and `billing.status`'s
/// ledgered emitted-call count.
pub fn utc_month_start_unix(now_unix: i64) -> i64 {
    let (y, m, _d) = civil_from_days(now_unix.div_euclid(86_400));
    days_from_civil(y, m, 1) * 86_400
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

/// PRD-mcphost-synthetic-flag requirement 2: `^[a-z0-9][a-z0-9:_-]{0,63}$` --
/// shared by the signup header path (`control::signup`) and both admin
/// retro-tag tools (`admin::tenant_set_synthetic`, `admin::tenants_set_synthetic`),
/// so a label is either valid everywhere it can be set or invalid
/// everywhere, never accepted by one path and stored malformed by another.
pub fn is_valid_synthetic_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    let len_ok = (1..=64).contains(&bytes.len());
    let first_ok = bytes
        .first()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    let rest_ok = bytes[1.min(bytes.len())..].iter().all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b':' || *b == b'_' || *b == b'-'
    });
    len_ok && first_ok && rest_ok
}

/// PRD-mcphost-tenant-attribution requirement 1: this host's own derived
/// read of where a signup came from -- independent of, but read alongside,
/// `tenants.synthetic`'s free-form harness label (migration 0008). `real`
/// on `/healthz` and in `mcphost funnel` means exactly `External`; every
/// other class counts as synthetic there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClass {
    Loopback,
    Fleet,
    External,
}

impl SourceClass {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceClass::Loopback => "loopback",
            SourceClass::Fleet => "fleet",
            SourceClass::External => "external",
        }
    }

    pub fn is_synthetic(self) -> bool {
        !matches!(self, SourceClass::External)
    }
}

impl std::fmt::Display for SourceClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// requirement 1: the source IP itself is loopback -- `"127.0.0.1"`,
/// `"::1"`, or anything else `std::net::IpAddr::is_loopback` recognizes.
/// An address this process can't parse (e.g. `handler::source_ip`'s
/// `"unknown"` fallback when axum's `ConnectInfo` is missing) is never
/// treated as loopback: an unclassifiable signup should read as external
/// and visible, not quietly disappear into the synthetic bucket.
pub fn is_loopback_source_ip(ip: &str) -> bool {
    ip.parse::<std::net::IpAddr>()
        .map(|a| a.is_loopback())
        .unwrap_or(false)
}

/// PRD-mcphost-client-ip-behind-proxy requirement 1: the single function
/// every per-source call site (the signup rate limiter, `signup_events`,
/// and any future `ConnectInfo`/`source_ip` reader) funnels through to
/// resolve "the source address" for a request. `peer_ip` is the TCP peer's
/// address as `handler::source_ip` reads it off axum's `ConnectInfo`
/// (`None` when the extractor found nothing -- the same case that string
/// has always reported as `"unknown"`); `forwarded_for` is the raw,
/// unparsed `X-Forwarded-For` header value, if the request carried one.
///
/// mcphost.dev runs behind Caddy on the same box, so the TCP peer of every
/// proxied request is loopback and only loopback -- this deployment trusts
/// that one proxy, never a configurable trusted-proxy list (see the PRD's
/// non-goals). A non-loopback peer's header is therefore never consulted
/// (requirement/AC2: a forged header from a real remote peer changes
/// nothing). When the peer is loopback and a header is present, the first
/// address in it (a proxy chain is `client, proxy1, proxy2, ...`) is parsed
/// as an `IpAddr`; an unparseable value falls back to the peer address,
/// logged once at debug level rather than rejecting the request (AC3) --
/// the same "never disappear a signup" posture as
/// [`is_loopback_source_ip`]'s own doc comment above.
pub fn resolve_source_ip(peer_ip: Option<&str>, forwarded_for: Option<&str>) -> String {
    let peer_str = peer_ip.unwrap_or("unknown").to_string();
    if !is_loopback_source_ip(&peer_str) {
        return peer_str;
    }
    let Some(header) = forwarded_for else {
        return peer_str;
    };
    let first = header.split(',').next().unwrap_or("").trim();
    match first.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.to_string(),
        Err(_) => {
            tracing::debug!(
                header = %header,
                "unparseable X-Forwarded-For from loopback peer; using peer address"
            );
            peer_str
        }
    }
}

/// requirement 1's "known fleet set": the fleet's own tenant
/// (`wintermute-hub`, by display name -- every tenant's namespace is
/// server-generated, so a human/deploy-script-driven signup can only name
/// itself via `display_name`) plus the two prefixes
/// `Db::probe_tenant_count` already uses for the harness's synthetic
/// personas (PRD-mcphost-tenant-delete's `panel_`/`probe-`) -- the same
/// rule, reused rather than reinvented, so "probe tenant" means one thing
/// across this crate.
pub fn is_known_fleet_display_name(display_name: &str) -> bool {
    display_name == "wintermute-hub"
        || display_name.starts_with("panel_")
        || display_name.starts_with("probe-")
}

/// Open question answered at build time: "should a tenant that signs up
/// from a cloud egress IP but with a synthorg `clientInfo.name` be
/// synthetic? Rule: explicit stamp or client name match ⇒ synthetic." A
/// `clientInfo.name` starting with `synthorg` (case-insensitive) matches --
/// this is exactly the case requirement 1's header defense exists for: a
/// future non-loopback harness that doesn't (yet) send the marker header
/// either.
pub fn is_known_synthorg_client(client_name: &str) -> bool {
    client_name.to_ascii_lowercase().starts_with("synthorg")
}

/// requirement 1's full derivation. `harness_marker_present` is the
/// `x-mcphost-synthetic` header's presence on the request (any value,
/// including one that later fails [`is_valid_synthetic_label`] and stores
/// no label) -- named here per the requirement's "name the exact header in
/// the build": no other header carrying "this is a harness/prober" exists
/// in this crate or `deploy/` today (grep confirms it), so the header
/// PRD-mcphost-synthetic-flag already defined for the explicit-stamp path
/// is reused as the marker rather than adding a second one. Its presence
/// classifies `Loopback` even from a non-loopback IP -- the class name is
/// the common case, not a literal claim about every request this branch
/// covers.
pub fn classify_source_class(
    source_ip: &str,
    harness_marker_present: bool,
    display_name: &str,
    client_name: Option<&str>,
) -> SourceClass {
    if is_loopback_source_ip(source_ip) || harness_marker_present {
        return SourceClass::Loopback;
    }
    if is_known_fleet_display_name(display_name)
        || client_name.is_some_and(is_known_synthorg_client)
    {
        return SourceClass::Fleet;
    }
    SourceClass::External
}

/// PRD-mcphost-provenance-audit requirement 1/2: the two-way real/synthetic
/// split every metrics surface now reports, derived from the same
/// `SourceClass`/`synthetic` classification `classify_source_class` already
/// computes at write time -- one function, reused by the live signup path,
/// the migration 0012 backfill rule, and (via the tenant's own
/// already-derived origin) the call-recording path, instead of three rules
/// that could drift (Technical considerations: "one contract, many
/// callers").
pub fn derive_origin(
    source_class: SourceClass,
    synthetic_label: Option<&str>,
) -> (&'static str, Option<String>) {
    if source_class.is_synthetic() {
        let detail = synthetic_label
            .map(str::to_string)
            .unwrap_or_else(|| source_class.as_str().to_string());
        ("synthetic", Some(detail))
    } else {
        let detail = synthetic_label
            .map(str::to_string)
            .unwrap_or_else(|| "external-unverified".to_string());
        ("external", Some(detail))
    }
}

/// PRD-mcphost-provenance-audit P1 requirement 5: loopback/private/public
/// triage for `signup_events.ip_class`, independent of `origin` (a fleet
/// signup from a public IP is still `synthetic` origin but `public` ip
/// class). IPv6 has no stable `is_private` in std; anything non-loopback
/// reports `public` for v6 (documented simplification).
pub fn classify_ip_class(ip: &str) -> &'static str {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            if v4.is_loopback() {
                "loopback"
            } else if v4.is_private() {
                "private"
            } else {
                "public"
            }
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            if v6.is_loopback() {
                "loopback"
            } else {
                "public"
            }
        }
        Err(_) => "public",
    }
}

/// `mcphost funnel --since <date>`'s `<date>` (a plain `YYYY-MM-DD`, UTC) --
/// unix seconds at that day's start, or `None` if it doesn't parse.
pub fn parse_date_ymd_unix(s: &str) -> Option<i64> {
    let mut parts = s.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400)
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
    fn synthetic_label_rules() {
        assert!(is_valid_synthetic_label("operator"));
        assert!(is_valid_synthetic_label("synthorg:mcp-host-project-gtm"));
        assert!(is_valid_synthetic_label("synthorg:backfill-20260906"));
        assert!(is_valid_synthetic_label("a"));
        assert!(is_valid_synthetic_label(&"a".repeat(64))); // max length
        assert!(!is_valid_synthetic_label("")); // empty
        assert!(!is_valid_synthetic_label(&"a".repeat(65))); // too long
        assert!(!is_valid_synthetic_label("Bad Label!")); // space, uppercase, punctuation
        assert!(!is_valid_synthetic_label(":leading-colon")); // must start alnum
        assert!(!is_valid_synthetic_label("-leading-dash"));
        assert!(!is_valid_synthetic_label("Synthorg:run")); // uppercase first char
    }

    #[test]
    fn source_class_derivation() {
        // AC1: loopback IP, no header -> Loopback.
        assert_eq!(
            classify_source_class("127.0.0.1", false, "Some Agent", None),
            SourceClass::Loopback
        );
        assert_eq!(
            classify_source_class("::1", false, "Some Agent", None),
            SourceClass::Loopback
        );
        // Header presence alone -> Loopback, even from a non-loopback IP.
        assert_eq!(
            classify_source_class("8.8.8.8", true, "Some Agent", None),
            SourceClass::Loopback
        );
        // Known fleet display name, non-loopback IP, no header -> Fleet.
        assert_eq!(
            classify_source_class("8.8.8.8", false, "wintermute-hub", None),
            SourceClass::Fleet
        );
        assert_eq!(
            classify_source_class("8.8.8.8", false, "panel_gtm_specialist_03", None),
            SourceClass::Fleet
        );
        assert_eq!(
            classify_source_class("8.8.8.8", false, "probe-smoke-test", None),
            SourceClass::Fleet
        );
        // Synthorg client name, non-loopback IP, no header, unknown display
        // name -> Fleet (the open question's resolution).
        assert_eq!(
            classify_source_class("8.8.8.8", false, "Real Sounding Name", Some("synthorg-runner")),
            SourceClass::Fleet
        );
        assert_eq!(
            classify_source_class("8.8.8.8", false, "Real Sounding Name", Some("SynthOrg")),
            SourceClass::Fleet
        );
        // AC4: non-loopback IP, no header, unknown client, unknown name -> External.
        assert_eq!(
            classify_source_class("8.8.8.8", false, "Joe's Real Company", Some("claude-code")),
            SourceClass::External
        );
        assert_eq!(
            classify_source_class("8.8.8.8", false, "Joe's Real Company", None),
            SourceClass::External
        );
    }

    #[test]
    fn source_class_is_synthetic() {
        assert!(!SourceClass::External.is_synthetic());
        assert!(SourceClass::Loopback.is_synthetic());
        assert!(SourceClass::Fleet.is_synthetic());
    }

    #[test]
    fn date_ymd_parsing() {
        assert_eq!(parse_date_ymd_unix("1970-01-01"), Some(0));
        assert_eq!(parse_date_ymd_unix("2026-09-08"), Some(1_788_825_600));
        assert_eq!(parse_date_ymd_unix("not-a-date"), None);
        assert_eq!(parse_date_ymd_unix("2026-13-01"), None); // bad month
        assert_eq!(parse_date_ymd_unix("2026-09-08-extra"), None);
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

    /// PRD-mcphost-sandbox-ready: known Unix timestamps against their known
    /// RFC 3339 UTC rendering -- the epoch itself, and a date past the
    /// civil-calendar algorithm's leap-year/century-boundary edges, so a
    /// transcription slip in `civil_from_days` fails loudly here rather
    /// than showing up as a subtly-wrong `sandbox_checked_at` on `/healthz`.
    #[test]
    fn rfc3339_from_unix_matches_known_timestamps() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(86_400), "1970-01-02T00:00:00Z");
        // 2000-02-29T00:00:00Z (a leap day on a century boundary that IS a
        // leap year, 2000 % 400 == 0).
        assert_eq!(rfc3339_from_unix(951_782_400), "2000-02-29T00:00:00Z");
        // Round-trips through the time-of-day fields too, not just the date.
        assert_eq!(rfc3339_from_unix(86_400 + 3661), "1970-01-02T01:01:01Z");
    }

    /// PRD-mcphost-metered-overage: `utc_month_start_unix` against known
    /// timestamps, including the same leap-year/century-boundary edge
    /// `rfc3339_from_unix_matches_known_timestamps` exercises for
    /// `civil_from_days`, since `days_from_civil` is its hand-derived
    /// inverse and deserves the same suspicion.
    #[test]
    fn utc_month_start_unix_matches_known_timestamps() {
        // The last day of February in a leap year -> 2000-02-01T00:00:00Z,
        // not into March.
        assert_eq!(utc_month_start_unix(951_782_400), 949_363_200);
        assert_eq!(
            rfc3339_from_unix(utc_month_start_unix(951_782_400)),
            "2000-02-01T00:00:00Z"
        );
        // The epoch itself is already a month start.
        assert_eq!(utc_month_start_unix(0), 0);
        // A December timestamp rolls the month start into December, not
        // into the next year.
        assert_eq!(
            rfc3339_from_unix(utc_month_start_unix(rfc3339_to_test_unix_dec_15_2025())),
            "2025-12-01T00:00:00Z"
        );
    }

    /// Test-only helper: 2025-12-15T00:00:00Z as a unix timestamp, derived
    /// independently of `civil_from_days`/`days_from_civil` (plain day
    /// arithmetic from the epoch) so the assertion above doesn't just check
    /// the two functions agree with each other.
    fn rfc3339_to_test_unix_dec_15_2025() -> i64 {
        // 2025-12-15 is 20,437 days after 1970-01-01 (55 years incl. 14
        // leap years * 365/366 days, plus Jan-Nov 2025 (334 days) + 14).
        20_437 * 86_400
    }

    /// PRD-mcphost-code-tools-warm-pool AC7: the 30th call in a tenant's
    /// window succeeds, the 31st does not; a different tenant's own window
    /// is unaffected.
    #[test]
    fn tool_run_limiter_allows_thirty_then_blocks() {
        let limiter = ToolRunLimiter::new();
        for i in 0..TOOL_RUN_RATE_LIMIT_PER_MINUTE {
            assert!(
                limiter.allow(1),
                "call {i} within the limit must be allowed"
            );
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

    /// PRD-grand-loop-billing: `utc_midnight_unix`/`next_utc_midnight_unix`
    /// against known timestamps -- exact midnight, and a time mid-day.
    #[test]
    fn utc_midnight_matches_known_timestamps() {
        assert_eq!(utc_midnight_unix(0), 0);
        assert_eq!(utc_midnight_unix(86_400), 86_400);
        assert_eq!(utc_midnight_unix(86_400 + 3661), 86_400);
        assert_eq!(next_utc_midnight_unix(86_400 + 3661), 2 * 86_400);
    }

    /// PRD-mcphost-runs-and-jobs requirement 1: 26 Crockford-base32
    /// characters, monotonically non-decreasing across calls made in the
    /// same or later millisecond (the leasing query's `ORDER BY id`
    /// correctness depends on this), and never repeats across a decent
    /// sample (the 80 random bits doing their job).
    #[test]
    fn new_ulid_is_26_chars_sortable_and_unique() {
        let mut ids = Vec::new();
        for _ in 0..200 {
            let id = new_ulid();
            assert_eq!(id.len(), 26, "ulid {id} is not 26 chars");
            assert!(
                id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
                "ulid {id} has unexpected chars"
            );
            ids.push(id);
        }
        let mut sorted = ids.clone();
        sorted.sort();
        // Two ids minted in the same millisecond can tie on their time
        // prefix and sort by their random suffix instead -- so this only
        // asserts every id is unique, not that mint order == sort order.
        let mut dedup = ids.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), ids.len(), "ulid collision in 200 draws");
        let _ = sorted;
    }
}
