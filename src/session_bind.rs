//! PRD-mcphost-session-bound-tenant-after-signup requirements 1/2/4/5: the
//! in-memory "this streamable-HTTP session created that tenant" map, and the
//! server-minted session identifier it is keyed by.
//!
//! **Why mcphost mints the identifier itself.** `rmcp` 3.3.0 only issues an
//! `Mcp-Session-Id` in `legacy_session_mode`, and flipping that on makes the
//! transport *require* the header back on every follow-up request -- which
//! the Claude Agent SDK's own recorded wire sequence never sends, so
//! `tests/compat_ac11_ac12_claude_sdk_replay.rs` would start failing with
//! HTTP 422 (that break is pinned, not assumed, by
//! `sessbind_ac01_*::enabling_rmcps_own_session_mode_would_break_claude_sdk_replay`).
//! So the transport stays stateless (`build_router` still runs
//! `with_legacy_session_mode(false)`) and this module supplies the session
//! identity one layer up, in `http::issue_session_id`: a client that echoes
//! the header gets continuity, a client that never sends it -- the SDK --
//! sees byte-for-byte today's behaviour.
//!
//! **Why the identifier is self-authenticating.** The PRD's non-functional
//! clause says session identifiers are "the transport's own (server-
//! generated, unguessable), never derived from client input", and its
//! technical considerations say "do not substitute a client-supplied
//! identifier". An id here is `<32 hex nonce>.<32 hex tag>` where the tag is
//! a truncated HMAC-SHA256 of the nonce under a per-process secret, so
//! [`SessionBindings::is_server_issued`] can reject anything this process
//! did not mint *without* keeping a row for every one-shot stateless request
//! that will never sign up. That is what keeps requirement 4's map bounded by
//! *bindings* (at most [`MAX_BOUND_SESSIONS`]) rather than by request volume.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use rand::RngCore;

/// The transport's own session header name (MCP streamable-HTTP), lowercase
/// because `http::HeaderName::from_static` requires it. Read off the request
/// and written onto the response by `http::issue_session_id`.
pub const SESSION_ID_HEADER: &str = "mcp-session-id";

/// The resolved, server-minted session id for one request, carried from
/// `http::issue_session_id` to `handler::call_tool` in the request's own
/// extensions (which `rmcp` forwards untouched inside
/// `http::request::Parts`, the same way `axum`'s `ConnectInfo` already
/// reaches `handler::source_ip`).
///
/// An extension rather than a rewritten request header on purpose: `rmcp`
/// reads `Mcp-Session-Id` off the request itself in `legacy_session_mode`,
/// and injecting one there would make that mode 404 every request as an
/// unknown session. Nothing mcphost adds to the request may change how the
/// transport underneath reads it.
#[derive(Debug, Clone)]
pub struct RequestSessionId(pub String);

/// Requirement 4: "at most 10,000 sessions, least-recently-used eviction".
pub const MAX_BOUND_SESSIONS: usize = 10_000;

/// Requirement 4: "24 h idle expiry".
pub const BINDING_IDLE_TTL_SECS: i64 = 24 * 60 * 60;

/// Bytes of randomness in a minted session id's nonce half.
const NONCE_BYTES: usize = 16;

/// Bytes of the HMAC tag kept in the id's second half. 16 bytes (128 bits)
/// is far beyond forgery range and keeps the header short.
const TAG_BYTES: usize = 16;

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// One session's binding: the tenant it created, plus the last time a
/// request on that session touched it (requirement 4's idle expiry).
#[derive(Debug, Clone, Copy)]
struct Binding {
    tenant_id: i64,
    last_used_unix: i64,
    /// Monotonic recency ticket -- the LRU order key in [`Inner::recency`].
    seq: u64,
}

#[derive(Default)]
struct Inner {
    bindings: HashMap<String, Binding>,
    /// `seq -> session id`, so the least-recently-used entry is
    /// `recency.first_key_value()` in O(log n) with no extra crate.
    recency: BTreeMap<u64, String>,
    next_seq: u64,
}

impl Inner {
    fn touch(&mut self, session_id: &str, now_unix: i64) {
        let seq = self.next_seq;
        self.next_seq += 1;
        if let Some(binding) = self.bindings.get_mut(session_id) {
            self.recency.remove(&binding.seq);
            binding.seq = seq;
            binding.last_used_unix = now_unix;
            self.recency.insert(seq, session_id.to_string());
        }
    }

    fn remove(&mut self, session_id: &str) {
        if let Some(binding) = self.bindings.remove(session_id) {
            self.recency.remove(&binding.seq);
        }
    }

    /// Requirement 4's bound: evict least-recently-used entries until the
    /// map is back under [`MAX_BOUND_SESSIONS`].
    fn evict_to_capacity(&mut self) {
        while self.bindings.len() > MAX_BOUND_SESSIONS {
            let Some((&seq, victim)) = self.recency.iter().next() else {
                break;
            };
            let victim = victim.clone();
            self.recency.remove(&seq);
            self.bindings.remove(&victim);
        }
    }
}

/// Requirement 4: "a binding lives only as long as the session ... and never
/// written to the database". In-memory only, `Arc`-shared across every
/// [`crate::state::AppState`] clone, the same rationale as
/// [`crate::billing::CheckoutSessionCache`].
#[derive(Clone)]
pub struct SessionBindings {
    secret: Arc<[u8; 32]>,
    inner: Arc<Mutex<Inner>>,
}

impl std::fmt::Debug for SessionBindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the signing secret.
        f.debug_struct("SessionBindings")
            .field("bound_sessions", &self.len())
            .finish()
    }
}

impl Default for SessionBindings {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionBindings {
    /// PRD-mcphost-session-bound-tenant-key requirement 1: a second (or
    /// third) independently-bounded binding map over the SAME session
    /// identity space as `other` -- an id `other.issue()` minted
    /// (`is_server_issued`/`bind`/`lookup` all check the HMAC tag against
    /// `self.secret`) validates here too, since this shares that exact
    /// secret, but starts with its own empty map: a `bind` here is
    /// invisible to `other.lookup`, and vice versa. Needed because
    /// `AppState::implicit_signup_memory`/`tenant_key_arg_memory` must
    /// remember a DIFFERENT fact than `AppState::session_bindings` (explicit
    /// signup/redeem) about the very same session id `http::issue_session_id`
    /// mints through `session_bindings.issue()` -- a fresh `SessionBindings::new()`
    /// would mint its own random secret and could never verify an id it
    /// didn't itself issue.
    pub fn new_sharing_secret(other: &SessionBindings) -> Self {
        Self {
            secret: other.secret.clone(),
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn new() -> Self {
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        Self {
            secret: Arc::new(secret),
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// HMAC-SHA256 of `data` under this process's secret -- the same
    /// per-process key that signs session ids, reused by `paged::Cursor` so a
    /// cursor, like a session id, dies with the process that minted it.
    pub fn mac(&self, data: &[u8]) -> Vec<u8> {
        crate::billing::hmac_sha256(self.secret.as_slice(), data).to_vec()
    }

    fn tag(&self, nonce_hex: &str) -> String {
        let mac = crate::billing::hmac_sha256(self.secret.as_slice(), nonce_hex.as_bytes());
        to_hex(&mac[..TAG_BYTES])
    }

    /// Mint a fresh, unguessable session identifier. Never touches the
    /// binding map -- an id only earns a map entry once a `signup`/redeem on
    /// it actually creates a tenant (requirement 5).
    pub fn issue(&self) -> String {
        let mut nonce = [0u8; NONCE_BYTES];
        rand::thread_rng().fill_bytes(&mut nonce);
        let nonce_hex = to_hex(&nonce);
        let tag = self.tag(&nonce_hex);
        format!("{nonce_hex}.{tag}")
    }

    /// Non-functional clause: "session identifiers are the transport's own
    /// (server-generated, unguessable), never derived from client input". A
    /// value this process did not mint -- a guess, a replay from another
    /// process, a client-chosen string -- fails here, and
    /// `http::issue_session_id` replaces it with a fresh minted id rather
    /// than letting it address a binding.
    pub fn is_server_issued(&self, session_id: &str) -> bool {
        let Some((nonce_hex, tag)) = session_id.split_once('.') else {
            return false;
        };
        if nonce_hex.len() != NONCE_BYTES * 2 || tag.len() != TAG_BYTES * 2 {
            return false;
        }
        if !nonce_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        crate::auth::constant_time_eq(self.tag(nonce_hex).as_bytes(), tag.as_bytes())
    }

    /// Requirement 1/2: bind (or rebind -- "a second `signup`/redeem on the
    /// same session rebinds it to the newest tenant") this session to a
    /// tenant it just created. Refuses an id this process did not mint.
    pub fn bind(&self, session_id: &str, tenant_id: i64, now_unix: i64) -> bool {
        if !self.is_server_issued(session_id) {
            return false;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        let seq = inner.next_seq;
        inner.next_seq += 1;
        if let Some(previous) = inner.bindings.insert(
            session_id.to_string(),
            Binding { tenant_id, last_used_unix: now_unix, seq },
        ) {
            inner.recency.remove(&previous.seq);
        }
        inner.recency.insert(seq, session_id.to_string());
        inner.evict_to_capacity();
        true
    }

    /// Requirement 1: the bound tenant id for this session, or `None` when
    /// the session never signed up, was evicted, or has been idle past
    /// [`BINDING_IDLE_TTL_SECS`] (requirement 4 -- an expired entry is
    /// dropped here rather than lingering until eviction).
    pub fn lookup(&self, session_id: &str, now_unix: i64) -> Option<i64> {
        if !self.is_server_issued(session_id) {
            return None;
        }
        let mut inner = self.inner.lock().ok()?;
        let binding = *inner.bindings.get(session_id)?;
        if now_unix - binding.last_used_unix >= BINDING_IDLE_TTL_SECS {
            inner.remove(session_id);
            return None;
        }
        inner.touch(session_id, now_unix);
        Some(binding.tenant_id)
    }

    /// Requirement 5: "if the bound tenant is disabled or deleted later ...
    /// the binding is dropped".
    pub fn drop_binding(&self, session_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.remove(session_id);
        }
    }

    /// Requirement 4 / AC8: how many sessions currently hold a binding.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|i| i.bindings.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_id_verifies_and_a_forged_one_does_not() {
        let bindings = SessionBindings::new();
        let id = bindings.issue();
        assert!(bindings.is_server_issued(&id));
        assert!(!bindings.is_server_issued("not-a-session"));
        assert!(!bindings.is_server_issued(&format!("{}.{}", "0".repeat(32), "0".repeat(32))));
        // A different process's secret never validates here.
        assert!(!SessionBindings::new().is_server_issued(&id));
    }

    #[test]
    fn a_shared_secret_instance_validates_the_originals_id_but_keeps_its_own_map() {
        let original = SessionBindings::new();
        let shared = SessionBindings::new_sharing_secret(&original);
        let id = original.issue();
        assert!(shared.is_server_issued(&id), "shares the original's secret");
        assert!(shared.bind(&id, 7, 0));
        assert_eq!(shared.lookup(&id, 0), Some(7));
        assert_eq!(original.lookup(&id, 0), None, "binding one map never binds the other");
    }

    #[test]
    fn binding_refuses_an_id_this_process_never_minted() {
        let bindings = SessionBindings::new();
        assert!(!bindings.bind("client-picked-this", 1, 0));
        assert_eq!(bindings.len(), 0);
        assert_eq!(bindings.lookup("client-picked-this", 0), None);
    }

    #[test]
    fn rebinding_the_same_session_keeps_one_entry() {
        let bindings = SessionBindings::new();
        let id = bindings.issue();
        assert!(bindings.bind(&id, 7, 100));
        assert!(bindings.bind(&id, 9, 101));
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings.lookup(&id, 102), Some(9));
    }

    #[test]
    fn an_idle_binding_expires_after_24h() {
        let bindings = SessionBindings::new();
        let id = bindings.issue();
        assert!(bindings.bind(&id, 7, 1_000));
        assert_eq!(bindings.lookup(&id, 1_000 + BINDING_IDLE_TTL_SECS - 1), Some(7));
        assert_eq!(bindings.lookup(&id, 1_000 + 2 * BINDING_IDLE_TTL_SECS), None);
        assert_eq!(bindings.len(), 0);
    }

    #[test]
    fn eviction_keeps_the_map_at_capacity_and_drops_the_least_recently_used() {
        let bindings = SessionBindings::new();
        let first = bindings.issue();
        assert!(bindings.bind(&first, 1, 0));
        let mut newest = String::new();
        for tenant_id in 2..=(MAX_BOUND_SESSIONS as i64 + 1) {
            newest = bindings.issue();
            assert!(bindings.bind(&newest, tenant_id, 0));
        }
        assert_eq!(bindings.len(), MAX_BOUND_SESSIONS);
        assert_eq!(bindings.lookup(&first, 0), None, "the oldest entry is the one evicted");
        assert_eq!(bindings.lookup(&newest, 0), Some(MAX_BOUND_SESSIONS as i64 + 1));
    }
}
