//! PRD-mcphost-elicitation-in-band-on-stateless-transport
//! AC5 (P0) — Given the suite, When run, Then every in-band test serves the
//! handler via `build_router` (no second router under `tests/`), a
//! sandbox-free contract test (no `require_user_namespaces_or_ci_skip`; fails
//! on GitHub CI) asserts prod's `(json_response, legacy_session_mode)` source
//! is `(true, false)` and built the in-band server, and a second asserts each
//! `/capabilities.json` row `true` for `elicitation`/`sampling` renders from
//! the in-band gate function and has a raw-client exerciser.

use crate::common;
use common::{TestServer, signup};
use mcphost::inband::{self, ElicitShape};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

async fn initialize(base: &str, key: &str, capabilities: Value) -> String {
    let resp = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {key}"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": capabilities,
            "clientInfo": {"name": "elib-ac05", "version": "1"},
        }}))
        .send()
        .await
        .expect("initialize");
    resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).expect("session id").to_string()
}

#[tokio::test]
async fn production_transport_source_is_stateless_json_first_and_built_the_in_band_server() {
    // The one named source, and what the production constructor reads.
    assert_eq!(inband::production_transport(), (true, false));
    let http_src = read("src/http.rs");
    assert!(
        http_src.contains(".with_json_response(crate::inband::PROD_JSON_RESPONSE)"),
        "build_router_with_session_mode must read json_response from inband::PROD_JSON_RESPONSE"
    );
    assert!(
        http_src.contains("build_router_with_session_mode(state, crate::inband::PROD_LEGACY_SESSION_MODE)"),
        "build_router must read legacy_session_mode from inband::PROD_LEGACY_SESSION_MODE"
    );
    assert!(
        http_src.contains("let app = build_router(state);"),
        "serve_on_listener_until (what TestServer and prod run) must serve build_router"
    );

    // `TestServer` is that constructor: an `initialize` through it fills the
    // in-band capability store under the minted session id.
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Elib AC5").await;
    let session = initialize(&server.base_url, &key, json!({"elicitation": {}, "sampling": {}})).await;
    let stored = server
        .state
        .session_bindings
        .inband()
        .lookup(&session, mcphost::state::now_unix())
        .expect("TestServer's router stored the initialize capabilities");
    assert_eq!(stored.elicitation, ElicitShape::Empty);
    assert!(stored.sampling);
    let stranger = server.state.session_bindings.inband().lookup("not-minted-here", mcphost::state::now_unix());
    assert!(stranger.is_none(), "an id the host did not mint reads as no capabilities");
}

#[test]
fn no_in_band_test_builds_a_router_prod_never_runs() {
    let needles = [
        ["Streamable", "HttpService"].concat(),
        ["legacy_", "session_mode"].concat(),
        ["build_router_with_", "session_mode"].concat(),
    ];
    let frames = [["elicitation", "/create"].concat(), ["sampling/", "createMessage"].concat()];
    let mut files = Vec::new();
    rust_files(&root().join("tests"), &mut files);
    let mut offenders = Vec::new();
    let mut in_band_files = 0;
    let this_file = Path::new(file!()).file_name().map(ToOwned::to_owned);
    for path in files {
        // This file names the needles in prose; it is the scanner, not a subject.
        if path.file_name().map(ToOwned::to_owned) == this_file {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap_or_default();
        let is_support = path.components().any(|c| c.as_os_str() == "support");
        let drives_a_frame = frames.iter().any(|f| src.contains(f.as_str())) || src.contains("elicit_session");
        if !(drives_a_frame || (is_support && src.contains("Mcp-Session-Id"))) {
            continue;
        }
        in_band_files += 1;
        if needles.iter().any(|n| src.contains(n.as_str())) {
            offenders.push(path.display().to_string());
        }
    }
    assert!(in_band_files >= 5, "the scan found only {in_band_files} in-band files");
    assert!(offenders.is_empty(), "in-band tests must serve build_router, not their own router: {offenders:?}");
    assert!(
        !read("tests/support/elicit_session.rs").contains(&["Streamable", "HttpService"].concat()),
        "tests/support/elicit_session.rs must not construct a router"
    );
}

/// Each `/capabilities.json` row the in-band gate decides, with the raw-client
/// exerciser file that drives its frame through `TestServer`.
const EXERCISERS: &[(&str, &str, &str)] = &[
    ("elicitation", "tests/elib_ac01_form_elicitation_rides_the_calls_own_stream.rs", "elicitation/create"),
    ("sampling", "tests/elib_ac02_bare_capability_and_sampling_ride_in_band.rs", "sampling/createMessage"),
];

#[tokio::test]
async fn capabilities_rows_for_both_kinds_render_from_the_gate_and_have_a_raw_client_exerciser() {
    let server = TestServer::start().await;
    let doc: Value = reqwest::get(format!("{}/capabilities.json", server.base_url))
        .await
        .expect("get")
        .json()
        .await
        .expect("json");
    let caps_src = read("src/capabilities.rs");
    for name in mcphost::capabilities::CLIENT_SIDE {
        let (_, exerciser, frame) = EXERCISERS
            .iter()
            .find(|(row, _, _)| row == name)
            .unwrap_or_else(|| panic!("client-side row `{name}` has no raw-client exerciser registered"));
        let advertised = doc["primitives"][name]["implemented"].as_bool().expect("implemented");
        assert_eq!(advertised, inband::in_band_available(), "row `{name}` must equal the in-band gate: {doc}");
        // The row's `implemented` is rendered from the gate, not a literal.
        let at = caps_src.find(&format!("name: \"{name}\",")).expect("row in capabilities.rs");
        let row = &caps_src[at..caps_src[at..].find("},").map_or(caps_src.len(), |e| at + e)];
        assert!(row.contains("implemented: crate::inband::in_band_available(),"), "row `{name}`: {row}");
        if advertised {
            let src = read(exerciser);
            assert!(src.contains(frame) && src.contains("TestServer"), "`{exerciser}` must drive `{frame}` through TestServer");
        }
    }
}
