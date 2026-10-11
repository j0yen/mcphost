//! PRD-mcphost-webhook-inbox-one-call R3: a short in-memory "this peer just
//! redeemed" map.
//!
//! `/mcp` is stateless, so a connection that completed `host.redeem` cannot
//! be bound to its tenant (`tests/sessbind_ac01_transport_stateless_*`). The
//! next key-less call from the same peer used to implicit-signup a SECOND
//! tenant. This map remembers, for [`PEER_TTL_SECS`], which namespace a peer
//! (client IP + User-Agent, hashed) just obtained, so that call is refused
//! with `tenant_key_missing` naming it instead.
//!
//! In memory only (a restart forgets it, which only ever re-opens implicit
//! signup), bounded at [`MAX_PEERS`], never holds a key.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// R3: "within the last 10 minutes".
pub const PEER_TTL_SECS: i64 = 600;

/// Upper bound on remembered peers; the oldest entry goes first.
pub const MAX_PEERS: usize = 10_000;

/// The map key for a peer: a hash of the client IP and User-Agent, so no
/// address or agent string is retained. (PRD open question: IP+UA, not IP
/// only, so two different agents behind one NAT rarely collide.)
pub fn peer_key(ip: &str, user_agent: Option<&str>) -> String {
    crate::auth::hash_key(&format!("{ip}\u{0}{}", user_agent.unwrap_or("")))
}

/// `peer key -> (namespace it obtained, when)`.
type PeerMap = HashMap<String, (String, i64)>;

#[derive(Clone, Default)]
pub struct PeerRecents {
    inner: Arc<Mutex<PeerMap>>,
}

impl std::fmt::Debug for PeerRecents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerRecents").field("peers", &self.len()).finish()
    }
}

impl PeerRecents {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember that `peer` obtained `namespace` at `at_unix`.
    pub fn record_at(&self, peer: &str, namespace: &str, at_unix: i64) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        map.insert(peer.to_string(), (namespace.to_string(), at_unix));
        if map.len() > MAX_PEERS {
            map.retain(|_, (_, at)| at_unix - *at < PEER_TTL_SECS);
        }
        while map.len() > MAX_PEERS {
            let Some(oldest) = map.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone()) else {
                break;
            };
            map.remove(&oldest);
        }
    }

    /// The namespace `peer` obtained within [`PEER_TTL_SECS`] of `now_unix`.
    pub fn lookup(&self, peer: &str, now_unix: i64) -> Option<String> {
        let mut map = self.inner.lock().ok()?;
        let (namespace, at) = map.get(peer)?.clone();
        if now_unix - at >= PEER_TTL_SECS {
            map.remove(peer);
            return None;
        }
        Some(namespace)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_is_remembered_for_ten_minutes_only() {
        let recents = PeerRecents::new();
        let peer = peer_key("203.0.113.9", Some("agent/1"));
        recents.record_at(&peer, "t_abc", 1_000);
        assert_eq!(recents.lookup(&peer, 1_000 + PEER_TTL_SECS - 1).as_deref(), Some("t_abc"));
        assert_eq!(recents.lookup(&peer, 1_000 + PEER_TTL_SECS), None);
        assert!(recents.is_empty(), "an expired entry is dropped on lookup");
    }

    #[test]
    fn user_agent_is_part_of_the_peer() {
        assert_ne!(peer_key("203.0.113.9", Some("a")), peer_key("203.0.113.9", Some("b")));
        assert_eq!(peer_key("203.0.113.9", None), peer_key("203.0.113.9", Some("")));
    }

    #[test]
    fn the_map_stays_bounded() {
        let recents = PeerRecents::new();
        for i in 0..(MAX_PEERS + 5) {
            recents.record_at(&format!("p{i}"), "t_x", 1_000);
        }
        assert_eq!(recents.len(), MAX_PEERS);
    }
}
