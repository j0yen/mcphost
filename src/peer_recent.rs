//! PRD-mcphost-webhook-inbox-one-call R3: a short in-memory "this peer just
//! redeemed" map.
//!
//! `/mcp` is stateless, so a connection that completed `host.redeem` cannot
//! be bound to its tenant (`tests/sessbind_ac01_transport_stateless_*`). The
//! next key-less call from the same peer used to implicit-signup a SECOND
//! tenant. This map remembers, for [`PEER_TTL_SECS`], which namespace a peer
//! (client IP + User-Agent, hashed) just obtained, so that call is refused
//! with `tenant_key_missing` instead. The record also carries the redeeming
//! request's host-minted session id, so a request that presents a DIFFERENT
//! host-minted session (another agent behind the same address) is not held
//! by it (PRD-mcphost-implicit-signup-per-session-not-per-peer).
//!
//! In memory only (a restart forgets it, which only ever re-opens implicit
//! signup), bounded at [`MAX_PEERS`], never holds a key.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// R3: "within the last 10 minutes".
pub const PEER_TTL_SECS: i64 = 600;

/// Upper bound on remembered peers; the oldest entry goes first.
pub const MAX_PEERS: usize = 10_000;

/// PRD-mcphost-implicit-signup-key-in-hand R2: the env var holding the number
/// of sessionless mints one peer may make inside [`PEER_TTL_SECS`].
pub const MINT_CAP_ENV: &str = "MCPHOST_IMPLICIT_PEER_MINT_CAP";

/// The cap when [`MINT_CAP_ENV`] is unset or unparsable.
pub const DEFAULT_MINT_CAP: usize = 50;

/// The cap [`MINT_CAP_ENV`] names: a positive integer, else the default.
/// `0` reads as `1` (the first mint of a peer is never refused by the cap).
pub fn parse_mint_cap(raw: Option<&str>) -> usize {
    match raw.map(str::trim).filter(|v| !v.is_empty()).map(str::parse::<usize>) {
        Some(Ok(cap)) => cap.max(1),
        _ => DEFAULT_MINT_CAP,
    }
}

/// The cap this process runs with: [`MINT_CAP_ENV`], else the default.
pub fn mint_cap_from_env() -> usize {
    parse_mint_cap(std::env::var(MINT_CAP_ENV).ok().as_deref())
}

/// The map key for a peer: a hash of the client IP and User-Agent, so no
/// address or agent string is retained. (PRD open question: IP+UA, not IP
/// only, so two different agents behind one NAT rarely collide.)
pub fn peer_key(ip: &str, user_agent: Option<&str>) -> String {
    crate::auth::hash_key(&format!("{ip}\u{0}{}", user_agent.unwrap_or("")))
}

/// What a peer's latest redeem or sessionless mint left behind.
#[derive(Debug, Clone, Default)]
struct PeerRecord {
    /// The tenant the peer obtained; empty while the record only holds the
    /// in-flight reservations of a peer that has minted nothing yet.
    namespace: String,
    /// The host-minted session id of the redeeming request (never a header
    /// value the host did not mint), so the gate can tell the redeeming
    /// session from another agent behind the same address.
    session_id: Option<String>,
    at_unix: i64,
    /// True when the tenant came from a sessionless implicit mint, whose
    /// client was handed its key in the envelope: such a peer is served up to
    /// the mint cap. A redeem record always holds (the client has its key).
    sessionless_mint: bool,
    /// Times of the mints counted toward the cap, oldest first, all inside
    /// the window of the last access.
    mints: Vec<i64>,
    /// Mints admitted by [`PeerRecents::reserve_mint`] and not yet recorded.
    pending: usize,
}

impl PeerRecord {
    fn prune(&mut self, now_unix: i64) {
        self.mints.retain(|t| now_unix - *t < PEER_TTL_SECS);
    }

    /// Seconds until the oldest counted mint leaves the window, 1..=600.
    fn retry_after_s(&self, now_unix: i64) -> u64 {
        let oldest = self.mints.first().copied().unwrap_or(now_unix);
        (oldest + PEER_TTL_SECS - now_unix).clamp(1, PEER_TTL_SECS) as u64
    }

    fn is_placeholder(&self) -> bool {
        self.namespace.is_empty() && self.mints.is_empty() && self.pending == 0
    }
}

/// `peer key -> its latest redeem or mint`.
type PeerMap = HashMap<String, PeerRecord>;

/// What the peer gate reads about a peer: the tenant it holds and its mint
/// count inside the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerState {
    pub namespace: String,
    pub session_id: Option<String>,
    /// The peer's tenant was minted sessionless and the client holds its key.
    pub sessionless_mint: bool,
    pub mints_in_window: usize,
    pub retry_after_s: u64,
}

/// A peer at the mint cap: what its refusal reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapReached {
    pub mints_in_window: usize,
    pub retry_after_s: u64,
}

#[derive(Clone)]
pub struct PeerRecents {
    inner: Arc<Mutex<PeerMap>>,
    cap: Arc<AtomicUsize>,
}

impl Default for PeerRecents {
    fn default() -> Self {
        Self { inner: Arc::default(), cap: Arc::new(AtomicUsize::new(DEFAULT_MINT_CAP)) }
    }
}

impl std::fmt::Debug for PeerRecents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerRecents").field("peers", &self.len()).field("cap", &self.cap()).finish()
    }
}

/// An admitted sessionless mint, counted against the cap until it is recorded
/// or dropped. Holding it is what keeps concurrent mints from one peer from
/// passing the cap together.
#[derive(Debug)]
pub struct MintReservation {
    recents: PeerRecents,
    peer: String,
}

impl Drop for MintReservation {
    fn drop(&mut self) {
        let Ok(mut map) = self.recents.inner.lock() else {
            return;
        };
        if let Some(record) = map.get_mut(&self.peer) {
            record.pending = record.pending.saturating_sub(1);
            if record.is_placeholder() {
                map.remove(&self.peer);
            }
        }
    }
}

impl PeerRecents {
    pub fn new() -> Self {
        Self::default()
    }

    /// A map whose cap is read from [`MINT_CAP_ENV`], once, at startup.
    pub fn from_env() -> Self {
        let recents = Self::default();
        recents.set_cap(mint_cap_from_env());
        recents
    }

    /// Sessionless mints one peer may make inside the window.
    pub fn cap(&self) -> usize {
        self.cap.load(Ordering::Relaxed)
    }

    pub fn set_cap(&self, cap: usize) {
        self.cap.store(cap.max(1), Ordering::Relaxed);
    }

    /// Remember that `peer` redeemed `namespace` at `at_unix`, on the
    /// host-minted session `session_id`. A redeem counts as a mint and the
    /// peer is held until the window elapses.
    pub fn record_at(&self, peer: &str, namespace: &str, session_id: Option<&str>, at_unix: i64) {
        self.record(peer, namespace, session_id, at_unix, false);
    }

    /// Remember that `peer` minted `namespace` implicitly without presenting a
    /// session (its client was handed the key in the envelope).
    pub fn record_sessionless_mint_at(&self, peer: &str, namespace: &str, session_id: Option<&str>, at_unix: i64) {
        self.record(peer, namespace, session_id, at_unix, true);
    }

    #[allow(clippy::too_many_arguments)]
    fn record(&self, peer: &str, namespace: &str, session_id: Option<&str>, at_unix: i64, sessionless_mint: bool) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        let record = map.entry(peer.to_string()).or_default();
        // A record past its window starts the count again.
        if !record.namespace.is_empty() && at_unix - record.at_unix >= PEER_TTL_SECS {
            record.mints.clear();
        }
        record.prune(at_unix);
        record.mints.push(at_unix);
        record.mints.sort_unstable();
        record.namespace = namespace.to_string();
        record.session_id = session_id.map(str::to_string);
        record.at_unix = at_unix;
        record.sessionless_mint = sessionless_mint;
        evict(&mut map, at_unix);
    }

    /// The namespace `peer` obtained within [`PEER_TTL_SECS`] of `now_unix`,
    /// and the session that obtained it.
    pub fn lookup(&self, peer: &str, now_unix: i64) -> Option<(String, Option<String>)> {
        self.state(peer, now_unix).map(|s| (s.namespace, s.session_id))
    }

    /// [`Self::lookup`] with the peer's mint count. An expired record is
    /// dropped, which resets the count.
    pub fn state(&self, peer: &str, now_unix: i64) -> Option<PeerState> {
        let mut map = self.inner.lock().ok()?;
        let record = map.get_mut(peer)?;
        if record.namespace.is_empty() {
            return None;
        }
        if now_unix - record.at_unix >= PEER_TTL_SECS {
            if record.pending == 0 {
                map.remove(peer);
            } else {
                record.namespace.clear();
                record.session_id = None;
                record.mints.clear();
            }
            return None;
        }
        record.prune(now_unix);
        Some(PeerState {
            namespace: record.namespace.clone(),
            session_id: record.session_id.clone(),
            sessionless_mint: record.sessionless_mint,
            mints_in_window: record.mints.len(),
            retry_after_s: record.retry_after_s(now_unix),
        })
    }

    /// Admit one sessionless mint for `peer`, or report that it is at the cap.
    /// Counting and admitting happen under one lock.
    pub fn reserve_mint(&self, peer: &str, now_unix: i64) -> Result<MintReservation, CapReached> {
        let cap = self.cap();
        let mut map = self.inner.lock().map_err(|_| CapReached { mints_in_window: 0, retry_after_s: 1 })?;
        let record = map.entry(peer.to_string()).or_default();
        if !record.namespace.is_empty() && now_unix - record.at_unix >= PEER_TTL_SECS {
            record.namespace.clear();
            record.session_id = None;
            record.mints.clear();
        }
        record.prune(now_unix);
        let counted = record.mints.len() + record.pending;
        if counted >= cap {
            let refused = CapReached { mints_in_window: counted, retry_after_s: record.retry_after_s(now_unix) };
            if record.is_placeholder() {
                map.remove(peer);
            }
            return Err(refused);
        }
        record.pending += 1;
        evict(&mut map, now_unix);
        Ok(MintReservation { recents: self.clone(), peer: peer.to_string() })
    }

    /// PRD-mcphost-implicit-signup-key-in-hand R1/AC2: a call that presented
    /// the key of the tenant `peer` just obtained has adopted it, so the
    /// record has nothing left to hold. Another tenant's key leaves it alone.
    pub fn forget_if_namespace(&self, peer: &str, namespace: &str) {
        if let Ok(mut map) = self.inner.lock()
            && map.get(peer).is_some_and(|r| r.namespace == namespace)
        {
            map.remove(peer);
        }
    }

    pub fn forget(&self, peer: &str) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(peer);
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Keep the map at [`MAX_PEERS`]: expired entries first, then the oldest.
fn evict(map: &mut PeerMap, now_unix: i64) {
    if map.len() > MAX_PEERS {
        map.retain(|_, r| r.pending > 0 || now_unix - r.at_unix < PEER_TTL_SECS);
    }
    while map.len() > MAX_PEERS {
        let Some(oldest) = map.iter().min_by_key(|(_, r)| r.at_unix).map(|(k, _)| k.clone()) else {
            break;
        };
        map.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_is_remembered_for_ten_minutes_only() {
        let recents = PeerRecents::new();
        let peer = peer_key("203.0.113.9", Some("agent/1"));
        recents.record_at(&peer, "t_abc", Some("s1"), 1_000);
        assert_eq!(
            recents.lookup(&peer, 1_000 + PEER_TTL_SECS - 1),
            Some(("t_abc".to_string(), Some("s1".to_string())))
        );
        assert_eq!(recents.lookup(&peer, 1_000 + PEER_TTL_SECS), None);
        assert!(recents.is_empty(), "an expired entry is dropped on lookup");
    }

    #[test]
    fn user_agent_is_part_of_the_peer() {
        assert_ne!(peer_key("203.0.113.9", Some("a")), peer_key("203.0.113.9", Some("b")));
        assert_eq!(peer_key("203.0.113.9", None), peer_key("203.0.113.9", Some("")));
    }

    #[test]
    fn the_cap_reads_a_positive_integer_else_the_default() {
        assert_eq!(parse_mint_cap(None), DEFAULT_MINT_CAP);
        assert_eq!(parse_mint_cap(Some("3")), 3);
        assert_eq!(parse_mint_cap(Some(" 7 ")), 7);
        assert_eq!(parse_mint_cap(Some("0")), 1, "the first mint is never refused by the cap");
        assert_eq!(parse_mint_cap(Some("many")), DEFAULT_MINT_CAP);
        assert_eq!(parse_mint_cap(Some("")), DEFAULT_MINT_CAP);
    }

    #[test]
    fn mints_count_inside_the_window_and_reset_when_it_elapses() {
        let recents = PeerRecents::new();
        recents.set_cap(3);
        let peer = peer_key("203.0.113.9", Some("agent/1"));
        for (i, at) in [1_000, 1_100, 1_200].into_iter().enumerate() {
            recents.record_sessionless_mint_at(&peer, &format!("t_{i}"), None, at);
        }
        let state = recents.state(&peer, 1_300).expect("recent");
        assert_eq!((state.mints_in_window, state.retry_after_s), (3, 300), "oldest leaves at 1600");
        assert!(state.sessionless_mint);
        let capped = recents.reserve_mint(&peer, 1_300).expect_err("at the cap");
        assert_eq!(capped, CapReached { mints_in_window: 3, retry_after_s: 300 });
        assert!(recents.state(&peer, 1_000 + PEER_TTL_SECS + 200).is_none(), "the window elapsed");
        assert!(recents.reserve_mint(&peer, 1_900).is_ok(), "the count restarts at 0");
    }

    #[test]
    fn reservations_count_until_dropped() {
        let recents = PeerRecents::new();
        recents.set_cap(2);
        let a = recents.reserve_mint("p", 10).expect("first");
        let b = recents.reserve_mint("p", 10).expect("second");
        assert_eq!(recents.reserve_mint("p", 10).expect_err("third").mints_in_window, 2);
        drop(a);
        let c = recents.reserve_mint("p", 10).expect("a dropped reservation frees its slot");
        drop((b, c));
        assert!(recents.is_empty(), "a placeholder with nothing pending is removed");
    }

    #[test]
    fn a_redeem_holds_and_a_key_in_use_frees() {
        let recents = PeerRecents::new();
        recents.record_at("p", "t_a", Some("s1"), 100);
        assert!(!recents.state("p", 101).expect("held").sessionless_mint);
        recents.forget_if_namespace("p", "t_other");
        assert!(recents.state("p", 101).is_some(), "another tenant's key leaves the record");
        recents.forget_if_namespace("p", "t_a");
        assert!(recents.state("p", 101).is_none());
    }

    #[test]
    fn the_map_stays_bounded() {
        let recents = PeerRecents::new();
        for i in 0..(MAX_PEERS + 5) {
            recents.record_at(&format!("p{i}"), "t_x", None, 1_000);
        }
        assert_eq!(recents.len(), MAX_PEERS);
    }
}
