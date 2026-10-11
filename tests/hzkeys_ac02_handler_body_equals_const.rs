//! PRD-mcphost-healthz-anon-keys-const AC2 (P0) -- the handler's anonymous
//! body is built by `http::anon_healthz_body`; this is the one test that
//! asserts its keys equal `ANON_HEALTHZ_KEYS` in-process, for both liveness
//! verdicts and over the live `/healthz`. When a sibling adds a key to the
//! builder without updating the const, the builder-level test fails with a
//! message naming `ANON_HEALTHZ_KEYS` (the HTTP-level tests elsewhere compare
//! to the const through `common::anon_healthz_keys()` and fail too, without
//! naming it).

use mcphost::build_info::{ANON_HEALTHZ_KEYS, BUILD};
use crate::common::TestServer;
use mcphost::http::{anon_healthz_body, anon_healthz_body_with, check_anon_healthz_keys};
use serde_json::json;

#[test]
fn anonymous_handler_body_keys_equal_anon_healthz_keys() {
    for ok in [true, false] {
        let body = anon_healthz_body(ok, &BUILD);
        if let Err(msg) = check_anon_healthz_keys(&body) {
            panic!("{msg}");
        }
        let mut keys: Vec<&str> = body.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ANON_HEALTHZ_KEYS, "ANON_HEALTHZ_KEYS vs handler body (ok={ok})");
    }
}

#[test]
fn a_handler_that_inserts_an_extra_key_fails_naming_the_const() {
    // The real builder body construction with one extra key inserted, i.e.
    // the handler after a sibling added a key without touching the const.
    let body = anon_healthz_body_with(true, &BUILD, |m| {
        m.insert("uptime_s".into(), json!(1));
    });
    let err = check_anon_healthz_keys(&body).expect_err("extra key must be rejected");
    assert!(err.contains("ANON_HEALTHZ_KEYS"), "message must name the const: {err}");
    assert!(err.contains("uptime_s"), "message must name the stray key: {err}");
}

#[tokio::test]
async fn live_anonymous_healthz_body_passes_the_const_check() {
    let server = TestServer::start().await;
    let body: serde_json::Value = reqwest::get(format!("{}/healthz", server.base_url))
        .await
        .expect("GET /healthz")
        .json()
        .await
        .expect("parse /healthz");
    if let Err(msg) = check_anon_healthz_keys(body.as_object().expect("object")) {
        panic!("{msg}");
    }
}
