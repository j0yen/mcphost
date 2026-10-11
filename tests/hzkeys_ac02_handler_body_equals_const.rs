//! PRD-mcphost-healthz-anon-keys-const AC2 (P0) -- the handler's anonymous
//! body is built by `http::anon_healthz_body`; this is the one test that
//! asserts its keys equal `ANON_HEALTHZ_KEYS`. When a sibling (or a fixture
//! copy of the builder) adds a key without updating the const, this test --
//! and only this one, since every other test compares to the const through
//! `common::anon_healthz_keys()` and never restates the set -- fails, with a
//! message naming `ANON_HEALTHZ_KEYS`.

use mcphost::build_info::{ANON_HEALTHZ_KEYS, BUILD};
use mcphost::http::{anon_healthz_body, check_anon_healthz_keys};
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
    // Fixture: the builder's output with one extra key, i.e. the handler
    // after a sibling added a key without touching the const.
    let mut body = anon_healthz_body(true, &BUILD);
    body.insert("uptime_s".into(), json!(1));
    let err = check_anon_healthz_keys(&body).expect_err("extra key must be rejected");
    assert!(err.contains("ANON_HEALTHZ_KEYS"), "message must name the const: {err}");
    assert!(err.contains("uptime_s"), "message must name the stray key: {err}");
}
