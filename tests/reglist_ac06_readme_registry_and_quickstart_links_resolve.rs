//! PRD-mcphost-registry-listing
//! AC6 (P1) -- Given the README, When a reader opens it, Then it contains
//! the registry link and the one-URL quickstart link, both resolving with
//! HTTP 200.
//!
//! Two halves, both proving a different part of AC6.
//!
//! The always-on half (`readme_names_registry_and_quickstart_links`) pins
//! the two links to the entry this repo actually publishes: the registry
//! link must be a `registry.modelcontextprotocol.io` query for the
//! published `name` from `registry/server.json`, its anchor text must be
//! that same name, and the quickstart link must be that entry's own
//! `websiteUrl`. A README that names some other host, drops a link, or
//! drifts from the published entry fails here with no network at all. No
//! YAML/HTML dependency added for the markdown parsing (same "hand-roll a
//! tiny parser over a file this repo owns" call
//! `tests/ci_sandbox_support/mod.rs::jobs` already makes for `ci.yml`).
//!
//! The "resolving with HTTP 200" half really does GET both links and is
//! Live, gated on `MCPHOST_LIVE=1` (same convention as
//! `tests/mcphost_team_memory_ac08_live_test_skipped_without_env.rs`): not
//! every environment that builds this crate has egress to
//! `registry.modelcontextprotocol.io`, so a normal `cargo test` must not
//! depend on it. It carries its own negative control -- a route that does
//! not exist on the same registry host must come back non-200 in the same
//! run -- so a green live result cannot come from a probe that calls
//! everything 200.
//!
//! `?search=` on the registry is cold-cache slow: the first request for a
//! given query has measured ~15s before any bytes arrive, every repeat
//! sub-second. The earlier version of this test used a flat 15s client
//! timeout and therefore failed on a cold cache with "error sending
//! request for url" -- which reads exactly like an environment with no
//! egress and is not. Hence the generous per-request timeout plus retry in
//! `resolve_status` below.

use std::sync::Mutex;
use std::time::Duration;

/// Per-request ceiling. Sized for the registry's cold-cache `?search=`
/// path (~15s measured), not for a healthy response (sub-second).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Transport-level attempts per URL before giving up. A cold cache can
/// stall past even the timeout above; a second attempt hits the warm path.
const ATTEMPTS: usize = 3;

fn readme() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The published registry entry this README is supposed to point at.
fn published_entry() -> serde_json::Value {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("registry/server.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// Every `[text](url)` pair on one markdown line, in order.
fn markdown_links(line: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(mid) = rest.find("](") {
        let (before, after) = (&rest[..mid], &rest[mid + 2..]);
        let Some(end) = after.find(')') else { break };
        // Anchor text is whatever follows the last unconsumed `[`.
        let text = match before.rfind('[') {
            Some(open) => &before[open + 1..],
            None => "",
        };
        out.push((text, &after[..end]));
        rest = &after[end + 1..];
    }
    out
}

/// `(registry anchor text, registry url, quickstart url)` off README's own
/// "Find it in the registry" line.
fn registry_and_quickstart_links() -> (String, String, String) {
    let readme = readme();
    let line = readme
        .lines()
        .find(|l| l.contains("Find it in the registry"))
        .expect("README must have a \"Find it in the registry\" line");

    let links = markdown_links(line);
    assert!(
        links.len() >= 2,
        "expected the registry link and the one-URL quickstart link on this line, got: {links:?}"
    );
    (
        links[0].0.to_string(),
        links[0].1.to_string(),
        links[1].1.to_string(),
    )
}

#[test]
fn readme_names_registry_and_quickstart_links() {
    let (registry_text, registry_link, quickstart_link) = registry_and_quickstart_links();
    let entry = published_entry();
    let name = entry["name"].as_str().expect("registry/server.json name");
    let website = entry["websiteUrl"]
        .as_str()
        .expect("registry/server.json websiteUrl");

    assert!(
        registry_link.starts_with("https://registry.modelcontextprotocol.io/"),
        "first link should be an https registry URL: {registry_link}"
    );
    assert_eq!(
        registry_text, name,
        "the registry link's anchor text should be the published entry name"
    );
    // The link has to actually look up *this* server, not just land on the
    // registry: either a `?search=` query carrying the published name's own
    // server part, or (PRD-mcphost-docs-external-links-resolve: the search
    // endpoint is cold-cache slow and tripped the link checker) the direct
    // `/servers/<percent-encoded published name>/...` lookup.
    let server_part = name.rsplit('/').next().unwrap_or(name);
    let direct = format!("/servers/{}", name.replace('/', "%2F"));
    let searched = registry_link
        .split_once("search=")
        .map(|(_, q)| q.split('&').next().unwrap_or(q).to_string());
    match searched {
        Some(q) => assert!(
            q.contains(server_part),
            "registry link should search for the published entry ({server_part}), \
             not {q:?}: {registry_link}"
        ),
        None => assert!(
            registry_link.contains(&direct),
            "registry link should query the registry by search or look up {direct}: {registry_link}"
        ),
    }
    assert_eq!(
        quickstart_link, website,
        "second link should be the one-URL quickstart, i.e. the published entry's websiteUrl"
    );
}

/// Pure predicate, deliberately not reading `std::env` itself, so the
/// skip-gate's behavior for an unset variable can be asserted
/// deterministically instead of depending on the ambient process
/// environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

fn should_run_live() -> bool {
    live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref())
}

/// Guards mutation of the process-global `MCPHOST_LIVE` env var: this test
/// binary's tests run on multiple threads by default, and both this file's
/// tests read/write that var.
static MCPHOST_LIVE_ENV_LOCK: Mutex<()> = Mutex::new(());

/// One real GET, retried only on a transport failure (timeout, reset, DNS
/// hiccup) -- never on a status code, so a non-200 answer is reported as
/// itself rather than retried away.
async fn resolve_status(client: &reqwest::Client, url: &str) -> reqwest::StatusCode {
    let mut last_err = None;
    for attempt in 1..=ATTEMPTS {
        match client.get(url).send().await {
            Ok(resp) => return resp.status(),
            Err(e) => {
                eprintln!("GET {url}: attempt {attempt}/{ATTEMPTS} failed: {e}");
                last_err = Some(e);
            }
        }
    }
    panic!(
        "GET {url}: no response in {ATTEMPTS} attempts ({:?}s each): {}",
        REQUEST_TIMEOUT.as_secs(),
        last_err.expect("loop ran at least once"),
    );
}

/// AC6 (Live) -- the two links on README's "Find it in the registry" line
/// both resolve with HTTP 200. Only executes with `MCPHOST_LIVE=1`;
/// otherwise this is a successful no-op.
///
/// The negative control in the same run is what makes the two 200s mean
/// something: a route that does not exist on the registry host must come
/// back non-200 through the same client and the same `resolve_status`
/// path, so "both links are 200" cannot be satisfied by a probe that
/// never distinguishes statuses.
#[tokio::test]
async fn registry_and_quickstart_links_resolve_200_when_live() {
    let should_run = {
        let _guard = MCPHOST_LIVE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        should_run_live()
    };
    if !should_run {
        eprintln!(
            "skipping live HTTP check: set MCPHOST_LIVE=1 to verify the registry and \
             quickstart links resolve with HTTP 200"
        );
        return;
    }

    let (_, registry_link, quickstart_link) = registry_and_quickstart_links();
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("build http client");

    for url in [&registry_link, &quickstart_link] {
        let status = resolve_status(&client, url).await;
        assert_eq!(status.as_u16(), 200, "{url} did not resolve with HTTP 200");
    }

    // Negative control: same host, same client, same API prefix as the
    // real registry link, on a route the registry does not serve.
    let broken = format!(
        "{}/v0/reglist-ac06-no-such-route",
        registry_link
            .split_once("/v0/")
            .map(|(origin, _)| origin.to_string())
            .unwrap_or_else(|| "https://registry.modelcontextprotocol.io".to_string())
    );
    let broken_status = resolve_status(&client, &broken).await;
    assert_ne!(
        broken_status.as_u16(),
        200,
        "{broken} answered 200, so this test's HTTP-200 assertion proves nothing"
    );
    eprintln!("negative control: {broken} -> {broken_status}");
}

#[test]
fn live_mode_is_disabled_when_mcphost_live_is_unset_or_not_1() {
    assert!(!live_mode_enabled(None), "unset must disable live mode");
    assert!(!live_mode_enabled(Some("")), "empty must disable live mode");
    assert!(!live_mode_enabled(Some("0")), "MCPHOST_LIVE=0 must disable live mode");
    assert!(!live_mode_enabled(Some("true")), "only the literal '1' enables live mode");
    assert!(live_mode_enabled(Some("1")), "MCPHOST_LIVE=1 must enable live mode");
}

/// Given `MCPHOST_LIVE` unset, the Live test above is skipped, not failed.
/// Exercises `should_run_live`, the same function the Live test itself
/// calls to decide whether to skip, against the *real* process environment
/// instead of a hand-fed literal.
///
/// Deliberately never sets `MCPHOST_LIVE=1` here: this test binary runs
/// its tests on multiple threads, including
/// `registry_and_quickstart_links_resolve_200_when_live` concurrently, and
/// flipping the var to "1" process-wide could make that test attempt a
/// real network call in an offline test run.
// Exclusive access to MCPHOST_LIVE held by MCPHOST_LIVE_ENV_LOCK
// (std::sync::Mutex) for this fn's whole body; the value is read then
// unconditionally restored before the guard drops.
// flake-lint: env-guarded
#[test]
fn mcphost_live_env_var_gates_the_real_skip_check() {
    let _guard = MCPHOST_LIVE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::var("MCPHOST_LIVE").ok();

    // SAFETY: exclusive access to MCPHOST_LIVE held by MCPHOST_LIVE_ENV_LOCK
    // for the lifetime of `_guard`; restored before it drops.
    unsafe {
        std::env::remove_var("MCPHOST_LIVE");
    }
    assert!(
        !should_run_live(),
        "with MCPHOST_LIVE unset in the real environment, the live test's own \
         skip-gate must evaluate to 'skip'"
    );

    // SAFETY: see above.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("MCPHOST_LIVE", v),
            None => std::env::remove_var("MCPHOST_LIVE"),
        }
    }
}
