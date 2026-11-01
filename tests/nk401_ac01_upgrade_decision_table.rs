//! PRD-mcphost-named-key-refusal-not-http-401
//! AC1 -- the extracted decision function `upgrade_for`: over every
//! (path class x error code x Reason) only (bare, tenant_key_missing,
//! ConnectionBound) and (bare, tenant_key_missing, PeerRecent) differ from
//! the cited tree, to no upgrade; the wire reason round-trips.

use mcphost::errors::Reason;
use mcphost::http::{PathClass, Upgrade, upgrade_for};

/// The cited tree's rule: the code alone decided.
fn cited(code: &str) -> Upgrade {
    match code {
        "tenant_key_missing" | "invalid_token" | "wrong_tenant" => Upgrade::To401,
        "insufficient_scope" => Upgrade::To403,
        _ => Upgrade::None,
    }
}

#[test]
fn nk401_ac01() {
    let reasons = [Reason::Anonymous, Reason::ConnectionBound("ns".into()), Reason::PeerRecent];
    let codes = ["tenant_key_missing", "invalid_token", "wrong_tenant", "insufficient_scope", "other"];
    let mut changed = vec![];
    for path in [PathClass::Bare, PathClass::Login, PathClass::Tenant] {
        for code in codes {
            for reason in &reasons {
                let got = upgrade_for(path, code, reason);
                if got != cited(code) {
                    changed.push((path, code, reason.wire(), got));
                }
            }
        }
    }
    assert_eq!(
        changed,
        vec![
            (PathClass::Bare, "tenant_key_missing", "connection_bound", Upgrade::None),
            (PathClass::Bare, "tenant_key_missing", "peer_recent", Upgrade::None),
        ]
    );
}

#[test]
fn nk401_ac01_wire_round_trips() {
    for r in [Reason::Anonymous, Reason::ConnectionBound("ns".into()), Reason::PeerRecent] {
        let back = Reason::from_wire(Some(r.wire()), Some("ns"));
        assert_eq!(back, r);
    }
    assert_eq!(Reason::from_wire(None, None), Reason::Anonymous);
    assert_eq!(Reason::from_wire(Some("whatever"), None), Reason::Anonymous);
}
