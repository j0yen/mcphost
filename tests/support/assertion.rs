//! Test-only identity-assertion fixtures for the `xaa_ac*.rs` suite
//! (PRD-mcphost-enterprise-managed-auth): a flexible signer that takes a
//! whole claims object (unlike `support/oauth.rs`'s `sign`, this suite's
//! tests need `email`, `jti`, and `offline_access` claims that helper has
//! no room for), reusing the same EC test keypairs `support/oauth.rs`
//! already generates.
#![allow(dead_code)]

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::Value;

/// Signs a full claims object as a compact JWS under `kid`, with
/// `priv_pem` -- `priv_pem` need not match `kid`'s published key (a
/// wrong-signature test case relies on exactly that mismatch, same as
/// `support/oauth.rs::sign`).
pub fn sign_assertion(kid: &str, priv_pem: &str, claims: &Value) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    let key = EncodingKey::from_ec_pem(priv_pem.as_bytes()).expect("valid EC PEM");
    encode(&header, claims, &key).expect("sign test assertion")
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs() as i64
}

/// A fresh, never-reused jti for a single test's assertion -- a ULID-ish
/// string is unnecessary here; uniqueness (not shape) is all any test
/// needs, including two calls within the same test in the same second.
pub fn fresh_jti() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}-{}", now_unix(), std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed))
}
