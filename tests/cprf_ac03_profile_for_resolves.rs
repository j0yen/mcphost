//! PRD-mcphost-client-profile-registry AC3 — `profile_for` resolves
//! `clientInfo` first, then `User-Agent`, else `UNKNOWN` with no source.

use mcphost::client_profiles::{From, UNKNOWN, profile_for};

#[test]
fn cursor_client_info_without_user_agent_resolves_to_cursor() {
    let (p, from) = profile_for(Some(("Cursor", "1.0")), None);
    assert_eq!(p.id, "cursor");
    assert_eq!(from, Some(From::ClientInfo));
}

#[test]
fn neither_signal_is_unknown_with_no_source() {
    let (p, from) = profile_for(None, None);
    assert_eq!(p, &UNKNOWN);
    assert_eq!(from, None);
    let (p, from) = profile_for(Some(("acme-agent", "9")), Some("curl/8"));
    assert_eq!((p.id, from), ("unknown", None));
}

#[test]
fn client_info_wins_over_user_agent() {
    let (p, from) = profile_for(Some(("Visual Studio Code", "1.99")), Some("cursor/1.2"));
    assert_eq!(p.id, "vscode");
    assert_eq!(from, Some(From::ClientInfo));
}

#[test]
fn user_agent_alone_resolves_when_client_info_is_unrecognised() {
    let (p, from) = profile_for(None, Some("cursor/1.2"));
    assert_eq!((p.id, from), ("cursor", Some(From::UserAgent)));
    let (p, from) = profile_for(Some(("acme", "1")), Some("Gemini-CLI/0.4"));
    assert_eq!((p.id, from), ("gemini_cli", Some(From::UserAgent)));
}
