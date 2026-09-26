//! PRD-mcphost-status-feed AC8 (P0): `www/status.html` fetches `/status.json`
//! and renders state, components with 90-day bars, and incidents; the
//! static fallback text remains when the fetch fails.
//!
//! `www/status.html` is a static file mcphost-deploy mirrors to Caddy's
//! static root (PRD's own technical considerations) -- the mcphost binary
//! itself never serves it, so there is no headless browser in this test
//! harness. Instead, `run_status_script` below extracts the page's real
//! inline `<script>` and runs it for real in `boa_engine` (a pure-Rust JS
//! engine, so no browser/C toolchain is needed on the runner box) against a
//! minimal mocked `document` and a mocked `fetch` that either resolves or
//! rejects like a real `/status.json` call would. This exercises the
//! actual success/failure branches of the page's own code, not a static
//! guess about what it probably does: `status_html_js_renders_live_status_
//! and_incidents_on_successful_fetch` proves a reachable `/status.json`
//! makes the page render all four components and the incident list, and
//! `status_html_js_keeps_fallback_visible_when_fetch_fails` proves a failed
//! fetch leaves the fallback visible and the live section hidden. The
//! remaining two tests keep the committed-markup contract (default-visible
//! fallback / default-hidden live section, so a client with JS disabled
//! still sees the fallback) and the `/status.json` response shape in sync.

use crate::common;

use boa_engine::{Context, Source};
use common::TestServer;

fn read_status_html() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("www/status.html"),
    )
    .expect("read www/status.html")
}

/// Extracts `www/status.html`'s real inline `<script>` body and runs it in a
/// fresh `boa_engine` context against a minimal mocked DOM and the given
/// `fetch` implementation, then reads back the DOM mutations it made.
fn run_status_script(html: &str, fetch_prelude: &str) -> serde_json::Value {
    let script_open = html.find("<script>").expect("page has a <script> tag") + "<script>".len();
    let script_close = html[script_open..].find("</script>").map(|i| script_open + i).expect("script close tag");
    let script_body = &html[script_open..script_close];

    let mut context = Context::default();

    // A `document` stand-in with exactly the five elements the page reads
    // off of, each with real getter/setter-backed `hidden`/`textContent`/
    // `innerHTML` properties so the page's own assignments are observable.
    let dom_prelude = r#"
        var __els = {};
        function __makeEl(id) {
            var e = { id: id, hidden: false };
            var _textContent = "";
            var _innerHTML = "";
            Object.defineProperty(e, "textContent", {
                get: function () { return _textContent; },
                set: function (v) { _textContent = v; }
            });
            Object.defineProperty(e, "innerHTML", {
                get: function () { return _innerHTML; },
                set: function (v) { _innerHTML = v; }
            });
            return e;
        }
        ["status-fallback", "status-live", "status-state", "status-components", "status-incidents"]
            .forEach(function (id) { __els[id] = __makeEl(id); });
        var document = { getElementById: function (id) { return __els[id]; } };
    "#;

    context.eval(Source::from_bytes(dom_prelude)).expect("install dom prelude");
    context.eval(Source::from_bytes(fetch_prelude)).expect("install fetch mock");
    context.eval(Source::from_bytes(script_body)).expect("run page script");
    // The page's fetch chain is asynchronous (real Promises); this drains
    // the microtask queue so its `.then`/`.catch` callbacks actually run.
    context.run_jobs().expect("drain microtask queue");

    let readback = r#"
        JSON.stringify({
            fallbackHidden: __els["status-fallback"].hidden,
            liveHidden: __els["status-live"].hidden,
            componentsHtml: __els["status-components"].innerHTML,
            incidentsHtml: __els["status-incidents"].innerHTML,
            stateHtml: __els["status-state"].innerHTML
        });
    "#;
    let result = context.eval(Source::from_bytes(readback)).expect("read back dom state");
    let json_str = result
        .as_string()
        .expect("readback evaluates to a JSON string")
        .to_std_string_escaped();
    serde_json::from_str(&json_str).expect("parse readback json")
}

#[test]
fn status_html_js_renders_live_status_and_incidents_on_successful_fetch() {
    let html = read_status_html();
    let mock_status = serde_json::json!({
        "state": "operational",
        "generated_at": 1_700_000_000i64,
        "components": [
            {"name": "mcp", "state": "ok", "uptime_90d": 0.9999},
            {"name": "exec", "state": "ok", "uptime_90d": 0.998},
            {"name": "db", "state": "ok", "uptime_90d": 1.0},
            {"name": "admin", "state": "ok", "uptime_90d": 0.997}
        ],
        "incidents_open": [
            {"title": "example open incident", "impact": "minor", "closed_at": null}
        ],
        "incidents_recent_30d": [
            {"title": "example resolved incident", "impact": "minor", "closed_at": 1_699_000_000i64}
        ]
    });
    let fetch_prelude = format!(
        r#"
        function fetch(url, opts) {{
            return Promise.resolve({{
                ok: true,
                status: 200,
                json: function () {{ return Promise.resolve({mock_status}); }}
            }});
        }}
        "#
    );

    let result = run_status_script(&html, &fetch_prelude);

    assert_eq!(
        result["fallbackHidden"], true,
        "a reachable /status.json must hide the fallback: {result}"
    );
    assert_eq!(
        result["liveHidden"], false,
        "a reachable /status.json must reveal the live section: {result}"
    );
    for name in ["mcp", "exec", "db", "admin"] {
        assert!(
            result["componentsHtml"].as_str().unwrap().contains(name),
            "rendered components must include {name}: {result}"
        );
    }
    assert!(
        result["incidentsHtml"].as_str().unwrap().contains("example open incident"),
        "rendered incidents must include the open incident: {result}"
    );
    assert!(
        result["incidentsHtml"].as_str().unwrap().contains("example resolved incident"),
        "rendered incidents must include the recent resolved incident: {result}"
    );
    assert!(
        result["stateHtml"].as_str().unwrap().contains("OPERATIONAL"),
        "rendered state must reflect the fetched state: {result}"
    );
}

#[test]
fn status_html_js_keeps_fallback_visible_when_fetch_fails() {
    let html = read_status_html();
    let fetch_prelude = r#"
        function fetch(url, opts) {
            return Promise.resolve({
                ok: false,
                status: 500,
                json: function () { return Promise.reject(new Error("no body")); }
            });
        }
    "#;

    let result = run_status_script(&html, fetch_prelude);

    assert_eq!(
        result["fallbackHidden"], false,
        "a failed /status.json fetch must keep the fallback visible: {result}"
    );
    assert_eq!(
        result["liveHidden"], true,
        "a failed /status.json fetch must keep the live section hidden: {result}"
    );
}

#[test]
fn status_html_has_a_visible_fallback_and_a_hidden_live_section() {
    let html = read_status_html();

    assert!(html.contains(r#"fetch("/status.json""#), "page must fetch /status.json");

    let fallback_open = html.find(r#"id="status-fallback""#).expect("status-fallback element");
    let fallback_tag_end = html[fallback_open..].find('>').map(|i| fallback_open + i).unwrap();
    assert!(
        !html[fallback_open..fallback_tag_end].contains("hidden"),
        "the fallback element must be visible by default (no `hidden` attribute), so a client \
         with JS disabled -- and, until the fetch resolves, every client -- still sees it: {}",
        &html[fallback_open..fallback_tag_end]
    );

    let live_open = html.find(r#"id="status-live""#).expect("status-live element");
    let live_tag_end = html[live_open..].find('>').map(|i| live_open + i).unwrap();
    assert!(
        html[live_open..live_tag_end].contains("hidden"),
        "the live element must start hidden -- only a successful fetch reveals it: {}",
        &html[live_open..live_tag_end]
    );
}

#[tokio::test]
async fn status_json_carries_every_field_the_page_reads() {
    let server = TestServer::start().await;
    mcphost::statusfeed::tick_once(&server.state)
        .await
        .expect("self-sample tick");
    mcphost::admin::incident_open(
        &server.state,
        &serde_json::json!({"title": "example", "impact": "minor", "components": ["mcp"]}),
    )
    .await
    .expect("admin.incident.open");

    let body = mcphost::statusfeed::status_json(&server.state)
        .await
        .expect("status_json");

    assert!(body["state"].is_string());
    assert!(body["generated_at"].is_i64());
    let components = body["components"].as_array().expect("components array");
    assert_eq!(components.len(), 4);
    for c in components {
        assert!(c["name"].is_string(), "component: {c}");
        assert!(c["state"].is_string(), "component: {c}");
        assert!(c.get("uptime_90d").is_some(), "component: {c}");
    }
    let incidents_open = body["incidents_open"].as_array().expect("incidents_open array");
    assert!(!incidents_open.is_empty());
    for i in incidents_open {
        assert!(i["title"].is_string(), "incident: {i}");
        assert!(i["impact"].is_string(), "incident: {i}");
        assert!(i.get("closed_at").is_some(), "incident: {i}");
    }
    assert!(body["incidents_recent_30d"].is_array());
}
