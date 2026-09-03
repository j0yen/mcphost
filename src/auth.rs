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

/// SHA-256 of a key, hex-encoded; the only form a key is ever stored in.
pub fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    to_hex(&hasher.finalize())
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
