//! PRD-mcphost-client-ip-behind-proxy
//! AC2 — Given a request whose peer is `198.51.100.4` and whose
//! `X-Forwarded-For` is `203.0.113.7`, When the handler resolves the
//! source, Then the source is `198.51.100.4`.
//!
//! `TestServer` (see `common/mod.rs`) binds to `127.0.0.1`, so no real
//! request in this test binary ever arrives with a non-loopback TCP peer --
//! there is no way to fake the OS-level peer address of a real connection
//! without standing up a second host. `state::resolve_source_ip` is
//! exactly the seam requirement 1 names for this ("a single function
//! resolves the source address"), so this AC is proven directly against
//! that public, pure function rather than through a full HTTP round trip;
//! `handler::source_ip` (the one call site, per requirement 4) does nothing
//! but extract `peer_ip`/`forwarded_for` and hand them to it unchanged.

#[test]
fn non_loopback_peer_ignores_forwarded_header() {
    let resolved = mcphost::state::resolve_source_ip(Some("198.51.100.4"), Some("203.0.113.7"));
    assert_eq!(
        resolved, "198.51.100.4",
        "a forged header from a non-loopback peer must change nothing (non-goal: no trusted-proxy list)"
    );
}

#[test]
fn non_loopback_peer_with_no_header_is_unaffected() {
    let resolved = mcphost::state::resolve_source_ip(Some("198.51.100.4"), None);
    assert_eq!(resolved, "198.51.100.4");
}
