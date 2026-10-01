//! Tenant key generation and hashing, and bearer-header extraction.

use rand::RngCore;
use sha2::{Digest, Sha256};

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// A random 32-byte URL-safe (hex) tenant key, shown to the caller once.
pub fn generate_key() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    to_hex(&bytes)
}

/// A namespace of the form `t_<8 hex>`.
pub fn generate_namespace() -> String {
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("t_{}", to_hex(&bytes))
}

/// PRD-mcphost-handoff-token requirement 1: a single-use handoff token
/// `signup(handoff: true)` returns in place of the raw tenant key. Same
/// entropy as [`generate_key`] (32 random bytes, hex) with a `ho_` prefix
/// so a token can never be mistaken for -- or accidentally accepted in
/// place of -- a real tenant key at any call site that only checks shape.
/// Stored (via [`hash_key`], same hash this module already uses for
/// tenant keys -- one hashing convention, not two) hashed, never in the
/// clear (see `db::Db::create_handoff_token`).
pub fn generate_handoff_token() -> String {
    format!("ho_{}", generate_key())
}

/// PRD-mcphost-url-bound-tenants requirement 2: a tenant's secret-URL
/// token -- 26-character Crockford base32 (same alphabet/packing
/// `state::new_ulid`'s own random half uses) of 128 random bits, with no
/// time component (unlike a ULID, this must never leak when its tenant
/// signed up). 128 bits don't divide evenly into 5-bit groups: the top 2
/// bits of the first character are always zero, costing nothing (the 128
/// bits of real randomness are unchanged) and matching exactly how
/// `state::new_ulid` already packs its own 80-bit random half into 16
/// characters, just scaled to one more byte. Stored hashed (`hash_key`,
/// same convention as a tenant key) as `tenants.url_secret_hash`, never in
/// the clear.
pub fn generate_url_secret() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut acc: u128 = 0;
    for b in bytes {
        acc = (acc << 8) | b as u128;
    }
    let mut out = String::with_capacity(26);
    for i in (0..26).rev() {
        let shift = i * 5;
        let idx = ((acc >> shift) & 0x1f) as usize;
        out.push(crate::state::CROCKFORD_ALPHABET[idx] as char);
    }
    out
}

/// SHA-256 of a key, hex-encoded; the only form a key is ever stored in.
pub fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    to_hex(&hasher.finalize())
}

/// Constant-time byte comparison, so a caller cannot learn a secret's
/// prefix from how long a comparison took. Lives here (rather than in one
/// of its callers) because both `handler::resolve_auth`'s admin-key check
/// and PRD-mcphost-session-bound-tenant-after-signup's session-id tag check
/// (`session_bind::SessionBindings::is_server_issued`) need exactly this.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Pull the bearer token out of an `Authorization: Bearer <key>` header.
pub fn extract_bearer(headers: &http::HeaderMap) -> Option<String> {
    let value = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let key = value.strip_prefix("Bearer ")?;
    if key.is_empty() {
        return None;
    }
    Some(key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_matches_shape() {
        let ns = generate_namespace();
        assert!(ns.starts_with("t_"));
        assert_eq!(ns.len(), 10);
        assert!(ns[2..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn url_secret_is_26_crockford_chars_and_unpredictable() {
        let a = generate_url_secret();
        let b = generate_url_secret();
        assert_eq!(a.len(), 26);
        assert!(a.chars().all(|c| crate::state::CROCKFORD_ALPHABET.contains(&(c as u8))));
        assert_ne!(a, b, "two generated secrets must not collide");
    }

    #[test]
    fn hash_is_deterministic_and_not_the_key() {
        let key = generate_key();
        let h1 = hash_key(&key);
        let h2 = hash_key(&key);
        assert_eq!(h1, h2);
        assert_ne!(h1, key);
    }

    #[test]
    fn extract_bearer_parses_header() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::AUTHORIZATION,
            "Bearer abc123".parse().unwrap(),
        );
        assert_eq!(extract_bearer(&headers).as_deref(), Some("abc123"));
    }

    #[test]
    fn extract_bearer_none_without_header() {
        let headers = http::HeaderMap::new();
        assert_eq!(extract_bearer(&headers), None);
    }
}
