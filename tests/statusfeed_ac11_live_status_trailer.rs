//! PRD-mcphost-status-feed
//! AC11 (P0, Live) -- Given prod after land, When
//! `https://mcphost.dev/status.json` is fetched, Then it returns
//! `operational` with four components and `status.html` renders it (proof:
//! the run's captured transcript, saved under `docs/receipts/<slug>.md`).
//!
//! This test **always runs**: it fetches `/status.json` over a real TCP
//! socket and checks the committed `www/status.html`'s render mechanism --
//! no mock, no stub. What the environment selects is only *which* endpoint
//! the JSON fetch drives (same split as
//! `tests/mcphost_tenant_data_export_ac04_live_export_trailer.rs` and
//! `tests/mcphost_tool_versions_ac06_live_rollback_trailer.rs`):
//!
//! * Default (every `cargo test` run, including this build's gate): a real
//!   server process on an ephemeral `127.0.0.1` port, serving the same
//!   `mcphost::http` app `main.rs` serves in production, ticked three times
//!   so all four components have a fresh ok sample.
//! * `MCPHOST_LIVE=1`: the identical `GET /status.json` request against the
//!   real `$MCPHOST_URL` (default `https://mcphost.dev`) -- the post-ship
//!   trailer run. It can only pass once this branch is deployed, since prod
//!   must actually serve `/status.json` as operational for the check to
//!   succeed.
//!
//! `www/status.html` is never served by the `mcphost` binary itself (a
//! static file `mcphost-deploy` mirrors to Caddy's static root, confirmed by
//! AC8's own test docs) -- there is no in-process HTTP route to fetch it
//! from a local `TestServer`, live or not. So the render check reads the
//! committed file directly on every run (the mechanism: it fetches
//! `/status.json` and has a live section to reveal), and the `MCPHOST_LIVE=1`
//! path additionally performs one real `GET $MCPHOST_URL/status.html` to
//! confirm the deploy actually publishes it, printing the response for the
//! trailer.
//!
//! The honest scope: the always-on path proves the mechanism end to end
//! against a real server built from this branch; it does not (and cannot,
//! before ship) prove that the *mcphost.dev deployment* answers with it.
//! The `MCPHOST_LIVE=1` path is exactly that second run.
//!
//! Env vars, read only when `MCPHOST_LIVE=1`:
//!   MCPHOST_URL  endpoint to run against (default: https://mcphost.dev)

use crate::common;

use common::{TempDataDir, TestServer, python_kind_registry};

/// Pure predicate, deliberately not reading `std::env` itself, so its
/// behavior for an unset variable is asserted deterministically instead of
/// depending on (and possibly mutating) the ambient process environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

struct Target {
    label: String,
    base_url: String,
    /// Kept alive for the test's duration in the default path; `None` when
    /// driving a real remote endpoint.
    _local: Option<TestServer>,
    /// Backs the default path's python kind registry (exec's sandbox
    /// path); `None` when driving a real remote endpoint.
    _envs_dir: Option<TempDataDir>,
}

impl Target {
    /// `MCPHOST_LIVE=1`: the real deployed host.
    fn live() -> Self {
        let base_url = std::env::var("MCPHOST_URL").unwrap_or_else(|_| "https://mcphost.dev".into());
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            label: format!("live {base_url}"),
            base_url,
            _local: None,
            _envs_dir: None,
        }
    }

    /// The default: a real `mcphost` server on a real loopback socket, with
    /// a python kind registered (so `exec`'s sandbox path has something to
    /// probe -- an unregistered kind counts as failing, per AC2), ticked
    /// three times so every component has a fresh ok sample -- the same "3
    /// minutes" convention AC1 uses.
    async fn local() -> Self {
        let envs_dir = TempDataDir::new();
        let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
        for _ in 0..3 {
            mcphost::statusfeed::tick_once(&server.state)
                .await
                .expect("self-sample tick");
        }
        Self {
            label: format!("server at {}", server.base_url),
            base_url: server.base_url.clone(),
            _local: Some(server),
            _envs_dir: Some(envs_dir),
        }
    }

    async fn resolve() -> Self {
        if live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref()) {
            Self::live()
        } else {
            Self::local().await
        }
    }
}

/// AC11's `/status.json` half -- runs against a real endpoint and asserts
/// `operational` with four components, printing the body for the ship
/// trailer. Runs on every `cargo test`; `MCPHOST_LIVE=1` only redirects it
/// at real prod.
#[tokio::test]
async fn status_json_is_operational_with_four_components() {
    let target = Target::resolve().await;

    let resp = reqwest::Client::new()
        .get(format!("{}/status.json", target.base_url))
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {}/status.json ({}): {e}", target.base_url, target.label));
    assert_eq!(resp.status(), 200, "target: {}", target.label);

    let body: serde_json::Value = resp.json().await.expect("parse /status.json");
    assert_eq!(body["state"], "operational", "target {}: {body}", target.label);
    let components = body["components"].as_array().expect("components array");
    assert_eq!(components.len(), 4, "target {}: {body}", target.label);

    // AC11's proof: the fetched status body, captured for the ship trailer.
    println!(
        "AC11 status.json proof ({}): {}",
        target.label,
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
    );
}

/// AC11's `status.html` half -- the committed page always has the fetch +
/// reveal mechanism AC8 locked; `MCPHOST_LIVE=1` additionally fetches the
/// real deployed page to confirm Caddy actually serves it.
#[tokio::test]
async fn status_html_has_the_render_mechanism_and_is_served_live() {
    let html = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("www/status.html"),
    )
    .expect("read www/status.html");
    assert!(html.contains(r#"fetch("/status.json""#), "page must fetch /status.json");
    assert!(html.contains(r#"id="status-live""#), "page must have a live section to reveal");

    if live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref()) {
        let base_url = std::env::var("MCPHOST_URL").unwrap_or_else(|_| "https://mcphost.dev".into());
        let base_url = base_url.trim_end_matches('/').to_string();
        let resp = reqwest::Client::new()
            .get(format!("{base_url}/status.html"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("GET {base_url}/status.html: {e}"));
        assert_eq!(resp.status(), 200, "live status.html must be served at {base_url}");
        let body = resp.text().await.expect("status.html body");
        assert!(
            body.contains(r#"id="status-live""#),
            "deployed status.html must carry the same live section: {body}"
        );
        println!("AC11 status.html live proof ({base_url}): served, {} bytes", body.len());
    }
}

#[test]
fn live_mode_is_disabled_when_mcphost_live_is_unset_or_not_1() {
    assert!(!live_mode_enabled(None), "unset must disable live mode");
    assert!(!live_mode_enabled(Some("")), "empty must disable live mode");
    assert!(
        !live_mode_enabled(Some("0")),
        "MCPHOST_LIVE=0 must disable live mode"
    );
    assert!(
        !live_mode_enabled(Some("true")),
        "only the literal '1' enables live mode"
    );
    assert!(
        live_mode_enabled(Some("1")),
        "MCPHOST_LIVE=1 must enable live mode"
    );
}
