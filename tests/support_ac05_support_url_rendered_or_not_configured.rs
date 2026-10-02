//! PRD-mcphost-first-hour-support-surface
//! AC5 (P0) — Given `MCPHOST_SUPPORT_URL` set to a test URL, When pages
//! render, Then `www/support.html`, README, quickstart, llms.txt and every
//! help page footer contain it; given it unset, Then they contain "support
//! channel not configured" and the status feed reports a warning.

use crate::common;
use common::TestServer;

use mcphost::help;

/// Serializes every test in this process that mutates `MCPHOST_SUPPORT_URL`
/// -- `/support.html` and `/help/<code>` read it fresh per request (see
/// `help.rs`'s module doc comment), so two tests racing on the same env var
/// in one `cargo test` binary would flake exactly the way
/// `tests/common/mod.rs`'s own `AdvisoryModeGuard`/`ADVISORY_MODE_LOCK`
/// pair does for `MCPHOST_ADVISORY_MODE` -- same `tokio::sync::Mutex`
/// (not `std::sync::Mutex`: this guard is held across `.await` points,
/// e.g. `TestServer::start().await`, which a std mutex guard isn't meant
/// to survive) and `Drop`-releases-together shape, copied here rather than
/// added to the shared harness since this PRD is the only user so far.
static SUPPORT_URL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct SupportUrlGuard {
    _permit: tokio::sync::MutexGuard<'static, ()>,
}

impl SupportUrlGuard {
    // `permit` excludes every other test touching MCPHOST_SUPPORT_URL for
    // this guard's whole lifetime (see SUPPORT_URL_LOCK's doc comment
    // above).
    // flake-lint: env-guarded
    async fn set(url: &str) -> Self {
        let permit = SUPPORT_URL_LOCK.lock().await;
        // SAFETY: `permit` excludes every other test touching this var for
        // this guard's whole lifetime.
        unsafe { std::env::set_var("MCPHOST_SUPPORT_URL", url) };
        Self { _permit: permit }
    }

    // Same exclusion as `set` above.
    // flake-lint: env-guarded
    async fn unset() -> Self {
        let permit = SUPPORT_URL_LOCK.lock().await;
        unsafe { std::env::remove_var("MCPHOST_SUPPORT_URL") };
        Self { _permit: permit }
    }
}

impl Drop for SupportUrlGuard {
    // Still holding `_permit` (see `set`/`unset` above), so this runs
    // under the same exclusion.
    // flake-lint: env-guarded
    fn drop(&mut self) {
        // SAFETY: see `set`/`unset` above -- still holding `_permit`.
        unsafe { std::env::remove_var("MCPHOST_SUPPORT_URL") };
    }
}

const TEST_URL: &str = "https://discord.gg/example-mcphost-support";

#[test]
fn support_line_contains_the_url_when_set() {
    let line = help::support_line(Some(TEST_URL));
    assert!(line.contains(TEST_URL), "{line}");
}

#[test]
fn support_line_names_not_configured_when_unset() {
    let line = help::support_line(None);
    assert!(line.contains("support channel not configured"), "{line}");
}

#[test]
fn render_support_page_contains_url_or_not_configured() {
    assert!(help::render_support_page(Some(TEST_URL)).contains(TEST_URL));
    assert!(help::render_support_page(None).contains("support channel not configured"));
}

#[test]
fn every_help_page_footer_carries_the_support_line() {
    for entry in help::HELP_ENTRIES {
        let page_set = help::render_help_page(entry, Some(TEST_URL));
        assert!(page_set.contains(TEST_URL), "{}: footer missing support url", entry.code);
        let page_unset = help::render_help_page(entry, None);
        assert!(
            page_unset.contains("support channel not configured"),
            "{}: footer missing not-configured phrase",
            entry.code
        );
    }
}

#[tokio::test]
async fn live_support_html_reflects_the_env_var_set_and_unset() {
    let _guard = SupportUrlGuard::set(TEST_URL).await;
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    let body = http
        .get(format!("{}/support.html", server.base_url))
        .send()
        .await
        .expect("GET /support.html")
        .text()
        .await
        .expect("body");
    assert!(body.contains(TEST_URL), "{body}");
    drop(_guard);

    let _guard2 = SupportUrlGuard::unset().await;
    let body2 = http
        .get(format!("{}/support.html", server.base_url))
        .send()
        .await
        .expect("GET /support.html")
        .text()
        .await
        .expect("body");
    assert!(body2.contains("support channel not configured"), "{body2}");
}

#[tokio::test]
async fn status_feed_warns_when_support_channel_unconfigured() {
    let _guard = SupportUrlGuard::unset().await;
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let status: serde_json::Value = http
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("status.json body");
    let warnings = status["warnings"].as_array().expect("warnings array");
    assert!(
        warnings.iter().any(|w| w.as_str().unwrap_or_default().contains("MCPHOST_SUPPORT_URL")),
        "status feed must warn when the support channel is unconfigured: {status}"
    );
}

#[tokio::test]
async fn status_feed_has_no_support_warning_when_configured() {
    let _guard = SupportUrlGuard::set(TEST_URL).await;
    let server = TestServer::start().await;
    let http = reqwest::Client::new();
    let status: serde_json::Value = http
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("status.json body");
    let warnings = status["warnings"].as_array().expect("warnings array");
    assert!(
        !warnings.iter().any(|w| w.as_str().unwrap_or_default().contains("MCPHOST_SUPPORT_URL")),
        "status feed must not warn once the support channel is configured: {status}"
    );
}

/// README/`docs/agent-quickstart.md`/`www/llms.txt` build-time half of AC5
/// -- these are plain repo docs, never served at request time, so
/// `MCPHOST_SUPPORT_URL`'s value for them is baked in at `mcphost gen-docs`
/// time (see `help.rs`'s module doc comment) and committed; today, with no
/// support channel chosen yet (open question in the PRD itself), that's the
/// "not configured" phrase in all three.
#[test]
fn committed_docs_carry_the_not_configured_phrase_today() {
    for (name, text) in [
        ("README.md", include_str!("../README.md")),
        ("docs/agent-quickstart.md", include_str!("../docs/agent-quickstart.md")),
        ("www/llms.txt", include_str!("../www/llms.txt")),
    ] {
        assert!(
            text.contains("support channel not configured"),
            "{name} must carry the support line (MCPHOST_SUPPORT_URL is unset today)"
        );
    }
}
