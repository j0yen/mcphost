//! PRD-mcphost-reachability-alt-host: a second public hostname
//! (`MCPHOST_ALT_PUBLIC_URLS`) that every human-facing link carries as a
//! fallback, plus the unauthenticated `/reach` probe and the shared
//! three-line template (requirement 5) every surface renders identically.
//!
//! Grounding (2026-10-05): a Comcast household's gateway classifier
//! (Xfinity xFi Advanced Security) flagged `mcphost.dev`'s SNI as a new,
//! unreputed domain and blocked the TLS handshake outright, redirecting
//! port 80 to `safebrowse.io`. One public hostname meant that single
//! classifier decision took every human-facing surface down at once
//! (claim link, docs, the MCP endpoint itself) -- this module is the
//! product's answer: a second, unrelated registrable domain every link
//! names as a fallback, and a content-free self-test any human or agent
//! can run without auth to tell which hostname actually works.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use serde_json::json;

use crate::state::AppState;

/// Requirement 1: `MCPHOST_ALT_PUBLIC_URLS` is a comma-separated list of
/// absolute base URLs (same shape as `MCPHOST_PUBLIC_URL`, e.g.
/// `https://alt.example`); blank entries (an empty var, a trailing comma,
/// stray whitespace) are dropped so an operator's unset-vs-empty-string env
/// quirk never turns into a spurious alt host -- an unset var yields the
/// empty list (AC5's "no `MCPHOST_ALT_PUBLIC_URLS`" case). Order is
/// preserved: the first entry is "the" alternate [`fallback_block`] quotes
/// (goal: "an `alt:` line with ... the first alternate host"). Split out
/// from [`alt_public_urls_from_env`] so a test can exercise the parsing
/// without mutating this process's actual environment -- every top-level
/// `tests/*.rs` file in this crate now links into one shared suite binary
/// (PRD-mcphost-test-suite-consolidation), where a global `set_var` in one
/// test is a cross-test race, not an isolated fixture.
pub fn parse_alt_public_urls(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
        .collect()
}

/// Reads `$MCPHOST_ALT_PUBLIC_URLS` once (same "read once at startup"
/// convention as `fleet_ips`/`verified_client_ids` in `main.rs`) and
/// parses it with [`parse_alt_public_urls`].
pub fn alt_public_urls_from_env() -> Vec<String> {
    parse_alt_public_urls(&std::env::var("MCPHOST_ALT_PUBLIC_URLS").unwrap_or_default())
}

/// The allow-steps page every fallback block points at: the www
/// `/cant-reach` page this binary is meant to serve alongside
/// `/status.html`/`/aup.html` (same `static_page`-over-`www/*.html`
/// pattern). Not yet wired to a route in this build -- see the PR's
/// deferred-ACs note; the path is still named here (rather than invented
/// per call site) so every template reference points at the same URL the
/// day the page exists.
pub const ALLOW_STEPS_PATH: &str = "/cant-reach";

/// Requirement 5: ONE template for the three-line fallback text (primary
/// link, `alt:` line, allow-steps URL) that the claim email, the invite
/// response, and `host.quickstart` all render from -- so the wording
/// cannot drift between surfaces (goal: "from one template"). `path` is
/// the route-relative path the primary link already points at (e.g.
/// `/claim/verify/<code>`); the alt line repeats that same path on the
/// first alternate host (AC1). Returns `None` when no alternates are
/// configured (AC5: no `alt:` line, behaviour matches today) -- every call
/// site simply skips appending anything in that case.
pub fn fallback_block(public_url: &str, alt_public_urls: &[String], path: &str) -> Option<String> {
    let first_alt = alt_public_urls.first()?;
    let alt_url = format!("{}{path}", first_alt.trim_end_matches('/'));
    let allow_steps_url = format!("{}{ALLOW_STEPS_PATH}", public_url.trim_end_matches('/'));
    Some(format!(
        "alt: {alt_url}\n\
         If neither link opens, your network's security filter may be blocking a new \
         domain -- see {allow_steps_url} for the one-click allow steps."
    ))
}

/// `GET /reach` (requirement 3 / AC3): unauthenticated, content-free, no
/// side effects -- an ISP classifier or a human can hit any public
/// hostname and get back proof the server behind it is alive, same
/// no-row-written contract `/healthz`'s anonymous body already has.
/// `host` echoes the request's own `Host` header (so a caller testing
/// several hostnames can tell which one it actually reached), falling
/// back to the configured primary when the header is absent or not valid
/// UTF-8 (a raw TCP health-checker with no Host header at all, say).
pub async fn reach(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| state.public_url.clone());
    Json(json!({
        "host": host,
        "ok": true,
        "served_at": crate::state::now_unix(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_alternates_yields_no_block() {
        assert_eq!(fallback_block("https://mcphost.dev", &[], "/claim/verify/abc"), None);
    }

    #[test]
    fn one_alternate_repeats_the_same_path() {
        let alts = vec!["https://alt.example".to_string()];
        let block = fallback_block("https://mcphost.dev", &alts, "/claim/verify/abc").unwrap();
        assert!(block.contains("alt: https://alt.example/claim/verify/abc"));
        assert!(block.contains("https://mcphost.dev/cant-reach"));
    }

    #[test]
    fn parsing_drops_blanks_and_trailing_slashes() {
        let urls = parse_alt_public_urls(" https://alt.example/ ,, https://second.example");
        assert_eq!(urls, vec!["https://alt.example", "https://second.example"]);
    }

    #[test]
    fn empty_input_yields_empty_list() {
        assert_eq!(parse_alt_public_urls(""), Vec::<String>::new());
    }
}
