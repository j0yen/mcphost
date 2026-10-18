//! PRD-mcphost-elicitation-in-band-on-stateless-transport: server→client
//! requests (`elicitation/create`, `sampling/createMessage`) on the
//! production transport.
//!
//! `build_router` runs rmcp's streamable-HTTP transport stateless and
//! JSON-first, so rmcp rebuilds every request's peer with
//! `ClientCapabilities::default()` and drops a POSTed client response
//! (`tower.rs:2018`). This module is the host's own half of the round trip,
//! one mechanism for both kinds:
//!
//! * [`InBandStores::remember`] keeps what a connection advertised at
//!   `initialize`, under the `Mcp-Session-Id` the host minted for it (R1),
//!   and [`elicitation_shape`] / [`sampling_advertised`] are the only readers
//!   of that advertisement (R2);
//! * [`ask`] sends the request on the call's own peer (rmcp falls back to an
//!   SSE reply when a handler's first message is a request) and awaits the
//!   client's answer on a oneshot registered under (session id, outbound
//!   request id) (R3);
//! * `http::deliver_client_response` hands the client's POSTed JSON-RPC
//!   response to that oneshot via [`InBandStores::deliver`] and answers `202`.
//!
//! The transport settings the production router is built with live here
//! ([`PROD_JSON_RESPONSE`], [`PROD_LEGACY_SESSION_MODE`]) so `build_router`
//! and the test that guards it read one source (R5), and the
//! `/capabilities.json` rows for both kinds are rendered from
//! [`in_band_available`] (R10).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::RoleServer;
use rmcp::model::{ClientCapabilities, RequestId, ServerRequest};
use rmcp::service::{Peer, PeerRequestOptions};
use serde_json::Value;
use tokio::sync::oneshot;

/// `StreamableHttpServerConfig::with_json_response` of the production router.
pub const PROD_JSON_RESPONSE: bool = true;

/// `with_legacy_session_mode` of the production router: `false`, because
/// rmcp's own session mode needs the header back on every request and the
/// Claude Agent SDK never sends it.
pub const PROD_LEGACY_SESSION_MODE: bool = false;

/// The `(json_response, legacy_session_mode)` the production router runs.
pub const fn production_transport() -> (bool, bool) {
    (PROD_JSON_RESPONSE, PROD_LEGACY_SESSION_MODE)
}

/// The gate behind `/capabilities.json`'s `implemented` for `elicitation` and
/// `sampling`: the in-band path exists only on the stateless JSON-first
/// transport, where rmcp falls back to SSE for a request-first handler and
/// the host's own response route delivers the answer.
pub const fn in_band_available() -> bool {
    PROD_JSON_RESPONSE && !PROD_LEGACY_SESSION_MODE
}

/// What a client advertised for elicitation at `initialize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElicitShape {
    /// `{"elicitation": {"form": ..}}`, with or without `url`.
    Form,
    /// The bare `{"elicitation": {}}`: form only, per MCP 2025-11-25.
    Empty,
    /// `{"elicitation": {"url": {}}}` alone: the link path.
    Url,
    /// `elicitation` absent.
    None,
}

impl ElicitShape {
    /// The log / audit name (`form` / `empty` / `url` / `none`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Form => "form",
            Self::Empty => "empty",
            Self::Url => "url",
            Self::None => "none",
        }
    }

    /// Form elicitation advertised (the bare `{}` counts).
    pub fn form(self) -> bool {
        matches!(self, Self::Form | Self::Empty)
    }
}

/// R2: the one reader of a client's elicitation advertisement.
pub fn elicitation_shape(caps: &ClientCapabilities) -> ElicitShape {
    match &caps.elicitation {
        None => ElicitShape::None,
        Some(e) if e.form.is_some() => ElicitShape::Form,
        Some(e) if e.url.is_some() => ElicitShape::Url,
        Some(_) => ElicitShape::Empty,
    }
}

/// R2: "form elicitation advertised".
pub fn form_elicitation_advertised(caps: &ClientCapabilities) -> bool {
    elicitation_shape(caps).form()
}

/// R2: "sampling advertised".
pub fn sampling_advertised(caps: &ClientCapabilities) -> bool {
    caps.sampling.is_some()
}

/// What the host remembers of one connection's `initialize`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Advertised {
    pub elicitation: ElicitShape,
    pub sampling: bool,
}

impl Advertised {
    pub fn of(caps: &ClientCapabilities) -> Self {
        Self { elicitation: elicitation_shape(caps), sampling: sampling_advertised(caps) }
    }

    fn nothing(self) -> bool {
        self.elicitation == ElicitShape::None && !self.sampling
    }
}

/// Capability entries kept (least-recently-used eviction beyond it).
pub const MAX_CAPABILITY_ENTRIES: usize = 10_000;

/// Idle expiry of a capability entry, like a signup binding.
pub const CAPABILITY_IDLE_TTL_SECS: i64 = 24 * 60 * 60;

/// In-flight in-band requests kept; past it a call takes the link path.
pub const MAX_INFLIGHT_ROUTES: usize = 10_000;

#[derive(Default)]
struct CapInner {
    entries: HashMap<String, (Advertised, i64, u64)>,
    /// `seq -> session id`: the oldest entry is the first key.
    recency: BTreeMap<u64, String>,
    next_seq: u64,
}

type Answer = Result<Value, Value>;

struct Slot {
    id: Option<RequestId>,
    tx: Option<oneshot::Sender<Answer>>,
}

/// The capability store (R1) and the response route (R3). Separate maps, so
/// an evicted capability entry never affects a call in flight. Both die with
/// the process, like the signup binding map.
pub struct InBandStores {
    caps: Mutex<CapInner>,
    ceiling: AtomicUsize,
    routes: Mutex<HashMap<String, Slot>>,
}

impl Default for InBandStores {
    fn default() -> Self {
        Self { caps: Mutex::default(), ceiling: AtomicUsize::new(MAX_CAPABILITY_ENTRIES), routes: Mutex::default() }
    }
}

impl InBandStores {
    /// Changes the capability-store ceiling (tests flood a small one).
    pub fn set_capability_ceiling(&self, ceiling: usize) {
        self.ceiling.store(ceiling.max(1), Ordering::SeqCst);
    }

    pub fn capability_ceiling(&self) -> usize {
        self.ceiling.load(Ordering::SeqCst)
    }

    /// Entries in the capability store.
    pub fn capability_len(&self) -> usize {
        self.caps.lock().map_or(0, |c| c.entries.len())
    }

    /// R1: remembers what `session_id` advertised at `initialize`. A client
    /// that advertised nothing earns no entry, so the store is bounded by
    /// clients that can use it. Evicts the oldest entries beyond the ceiling.
    pub fn remember(&self, session_id: &str, caps: &ClientCapabilities, now_unix: i64) {
        let advertised = Advertised::of(caps);
        let Ok(mut inner) = self.caps.lock() else { return };
        let previous = if advertised.nothing() {
            inner.entries.remove(session_id)
        } else {
            let seq = inner.next_seq;
            inner.next_seq += 1;
            inner.recency.insert(seq, session_id.to_string());
            inner.entries.insert(session_id.to_string(), (advertised, now_unix, seq))
        };
        if let Some((_, _, old)) = previous {
            inner.recency.remove(&old);
        }
        let ceiling = self.capability_ceiling();
        while inner.entries.len() > ceiling {
            let Some((&seq, victim)) = inner.recency.iter().next() else { break };
            let victim = victim.clone();
            inner.recency.remove(&seq);
            inner.entries.remove(&victim);
        }
    }

    /// R1: what `session_id` advertised, or `None` (never `initialize`d on
    /// this process, evicted, idle past the TTL) -- the caller then sees
    /// today's `ClientCapabilities::default()`.
    pub fn lookup(&self, session_id: &str, now_unix: i64) -> Option<Advertised> {
        let mut inner = self.caps.lock().ok()?;
        let (advertised, last, seq) = *inner.entries.get(session_id)?;
        inner.recency.remove(&seq);
        if now_unix.saturating_sub(last) > CAPABILITY_IDLE_TTL_SECS {
            inner.entries.remove(session_id);
            return None;
        }
        let fresh = inner.next_seq;
        inner.next_seq += 1;
        inner.recency.insert(fresh, session_id.to_string());
        inner.entries.insert(session_id.to_string(), (advertised, now_unix, fresh));
        Some(advertised)
    }

    /// Drops a session's stored capabilities (the client ended it).
    pub fn forget(&self, session_id: &str) {
        if let Ok(mut inner) = self.caps.lock()
            && let Some((_, _, seq)) = inner.entries.remove(session_id)
        {
            inner.recency.remove(&seq);
        }
    }

    /// R3: claims the session's one in-band slot, or `None` when it already
    /// has a request in flight (stateless mode gives every call's outbound
    /// request id 0, so a second would be ambiguous) or the route map is full.
    fn reserve(self: &Arc<Self>, session_id: &str) -> Option<Reservation> {
        let mut routes = self.routes.lock().ok()?;
        if routes.contains_key(session_id) || routes.len() >= MAX_INFLIGHT_ROUTES {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        routes.insert(session_id.to_string(), Slot { id: None, tx: Some(tx) });
        Some(Reservation { stores: self.clone(), session: session_id.to_string(), rx })
    }

    /// Whether `session_id` has an in-band request in flight (the cheap test
    /// that keeps every other POST from having its body read).
    pub fn has_inflight(&self, session_id: &str) -> bool {
        self.routes.lock().is_ok_and(|r| r.contains_key(session_id))
    }

    /// Delivers a client's JSON-RPC response to the waiter registered under
    /// (`session_id`, `id`). `false` -- and nothing delivered -- for an
    /// unknown or already-completed id or a session with nothing in flight.
    pub fn deliver(&self, session_id: &str, id: &RequestId, answer: Answer) -> bool {
        let Ok(mut routes) = self.routes.lock() else { return false };
        let Some(slot) = routes.get_mut(session_id) else { return false };
        if slot.id.as_ref() != Some(id) {
            return false;
        }
        slot.tx.take().is_some_and(|tx| tx.send(answer).is_ok())
    }
}

/// A claimed in-band slot; released when dropped.
struct Reservation {
    stores: Arc<InBandStores>,
    session: String,
    rx: oneshot::Receiver<Answer>,
}

impl Reservation {
    fn bind(&self, id: RequestId) {
        if let Ok(mut routes) = self.stores.routes.lock()
            && let Some(slot) = routes.get_mut(&self.session)
        {
            slot.id = Some(id);
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut routes) = self.stores.routes.lock() {
            routes.remove(&self.session);
        }
    }
}

/// Why an in-band request returned no answer.
#[derive(Debug)]
pub enum AskError {
    /// The session already has one in flight, or the route map is full.
    Busy,
    /// The client did not answer within the wall.
    Timeout,
    /// The client answered with a JSON-RPC error.
    Client(String),
    /// The request could not be sent.
    Send(String),
}

/// R3: sends `request` to the client on `peer` and returns its JSON `result`.
/// The answer arrives through [`InBandStores::deliver`], not rmcp's peer.
pub async fn ask(
    stores: &Arc<InBandStores>,
    peer: &Peer<RoleServer>,
    session_id: &str,
    request: ServerRequest,
    wall: Duration,
) -> Result<Value, AskError> {
    let mut reservation = stores.reserve(session_id).ok_or(AskError::Busy)?;
    let handle = peer
        .send_request_with_option(request, PeerRequestOptions::no_options())
        .await
        .map_err(|e| AskError::Send(e.to_string()))?;
    reservation.bind(handle.id.clone());
    match tokio::time::timeout(wall, &mut reservation.rx).await {
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(error))) => Err(AskError::Client(error.to_string())),
        Ok(Err(_)) => Err(AskError::Send("the response route closed".into())),
        Err(_) => {
            // Tell the client to stop waiting; it must not delay the result.
            let _ = tokio::time::timeout(Duration::from_secs(1), handle.cancel(Some("request timeout".into()))).await;
            Err(AskError::Timeout)
        }
    }
}

/// A client's POSTed JSON-RPC response (`result` or `error`), if `body` is
/// one: its request id and its outcome.
pub fn client_response(body: &[u8]) -> Option<(RequestId, Answer)> {
    let doc: Value = serde_json::from_slice(body).ok()?;
    let obj = doc.as_object()?;
    if obj.contains_key("method") {
        return None;
    }
    let id: RequestId = serde_json::from_value(obj.get("id")?.clone()).ok()?;
    if let Some(result) = obj.get("result") {
        return Some((id, Ok(result.clone())));
    }
    obj.get("error").map(|error| (id, Err(error.clone())))
}

/// What one `tools/call` may do in-band, read once from the stored
/// advertisement (R2, R7).
#[derive(Clone, Debug)]
pub struct CallGate {
    pub session_id: Option<String>,
    /// The request presented a session id the host minted (R7).
    pub session_present: bool,
    pub elicit: bool,
    pub sampling: bool,
    pub shape: ElicitShape,
}

impl CallGate {
    /// `per_request_caps`: the call carried its own `_meta` capabilities,
    /// which a stateless peer cannot answer, so it stays off the channel.
    pub fn read(
        stores: &InBandStores,
        session_id: Option<&str>,
        session_present: bool,
        per_request_caps: bool,
        now_unix: i64,
    ) -> Self {
        let advertised = session_id.and_then(|s| stores.lookup(s, now_unix));
        let shape = advertised.map_or(ElicitShape::None, |a| a.elicitation);
        let usable = in_band_available() && session_id.is_some() && !per_request_caps;
        Self {
            session_id: session_id.map(str::to_string),
            session_present,
            elicit: usable && shape.form(),
            sampling: usable && advertised.is_some_and(|a| a.sampling),
            shape,
        }
    }
}

tokio::task_local! {
    /// Set by `McpHostHandler::call_tool` for one dispatch: what the call's
    /// client advertised, for the structured log lines (R7).
    pub static CALL_GATE: CallGate;
}

/// The gate of the `tools/call` being dispatched, if it has one.
pub fn current_gate() -> Option<CallGate> {
    CALL_GATE.try_with(Clone::clone).ok()
}

/// R7: one structured line per `mcphost.elicit` / `mcphost.sampling.ask`
/// naming the `channel` that carried it (`client` in-band, `link` for
/// elicitation's ask page, `none` for a refused sampling ask), whether the
/// request presented a minted session id, and what the client advertised --
/// so a real client's behaviour reads from prod logs without a packet
/// capture. `outcome` is the result's action or the error code; never the
/// message, prompt or answer.
pub fn log_ask(kind: &'static str, gate: Option<&CallGate>, channel: &'static str, outcome: &str) {
    let session_present = gate.is_some_and(|g| g.session_present);
    let shape = gate.map_or(ElicitShape::None, |g| g.shape).as_str();
    // `session_unknown`: an id was presented but this process never stored it
    // (a restart, an eviction, a client that advertised nothing).
    let session_unknown = gate.is_some_and(|g| g.session_present && g.shape == ElicitShape::None && !g.sampling);
    match kind {
        "sampling" => tracing::info!(channel, session_present, session_unknown, capability_shape = shape, outcome, "sampling"),
        _ => tracing::info!(channel, session_present, session_unknown, capability_shape = shape, outcome, "elicit"),
    }
}

#[cfg(test)]
mod inbandsec_tests {
    use super::*;
    use crate::session_bind::SessionBindings;

    fn waiter(stores: &Arc<InBandStores>, session: &str, id: i64) -> Reservation {
        let reservation = stores.reserve(session).expect("slot free");
        reservation.bind(RequestId::Number(id as _));
        reservation
    }

    fn answer() -> Answer {
        Ok(serde_json::json!({"action": "accept"}))
    }

    #[test]
    fn inbandsec_deliver_is_keyed_by_session_and_request_id() {
        let bindings = SessionBindings::new();
        let stores = Arc::new(InBandStores::default());
        let (s1, s2) = (bindings.issue(), bindings.issue());
        assert_ne!(s1, s2);
        let mut w1 = waiter(&stores, &s1, 7);

        // Another server-issued session, same request id: refused, waiter untouched.
        assert!(!stores.deliver(&s2, &RequestId::Number(7 as _), answer()));
        assert!(w1.rx.try_recv().is_err(), "a foreign session must not resolve the waiter");
        // Right session, wrong request id: refused, waiter untouched.
        assert!(!stores.deliver(&s1, &RequestId::Number(8 as _), answer()));
        assert!(w1.rx.try_recv().is_err(), "a different request id must not resolve the waiter");
        // Right session and id: delivered exactly once.
        assert!(stores.deliver(&s1, &RequestId::Number(7 as _), answer()));
        assert_eq!(w1.rx.try_recv().expect("resolved"), answer());
        assert!(!stores.deliver(&s1, &RequestId::Number(7 as _), answer()), "a second answer is refused");
    }

    #[test]
    fn inbandsec_a_waiter_in_another_session_is_not_resolved_by_a_cross_answer() {
        let bindings = SessionBindings::new();
        let stores = Arc::new(InBandStores::default());
        let (s1, s2) = (bindings.issue(), bindings.issue());
        let mut w1 = waiter(&stores, &s1, 7);
        let mut w2 = waiter(&stores, &s2, 7);
        assert!(stores.deliver(&s2, &RequestId::Number(7 as _), answer()));
        assert!(w2.rx.try_recv().is_ok());
        assert!(w1.rx.try_recv().is_err(), "S2's answer never reaches S1's waiter");
    }

    #[test]
    fn inbandsec_a_session_with_nothing_in_flight_takes_no_answer() {
        let bindings = SessionBindings::new();
        let stores = Arc::new(InBandStores::default());
        let s1 = bindings.issue();
        assert!(!stores.has_inflight(&s1));
        assert!(!stores.deliver(&s1, &RequestId::Number(7 as _), answer()));
        let w = waiter(&stores, &s1, 7);
        assert!(stores.has_inflight(&s1));
        drop(w);
        assert!(!stores.has_inflight(&s1), "a dropped reservation frees the slot");
    }
}
