//! AES-256-GCM encryption for tenant secret values.
//!
//! `$MCPHOST_SECRET_KEY` is an operator-supplied string of any length; it is
//! SHA-256-hashed to a 32-byte AES-256 key so operators never have to hand
//!-generate exactly 32 bytes.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::errors::AppError;

/// Redact by key name, recursively, anywhere in `value`'s tree: every
/// object entry whose key is in `keys` has its value replaced with `"***"`,
/// at any depth, regardless of what that value actually is.
///
/// This is deliberately a different mechanism from `kinds::http`'s and
/// `kinds::python`'s own `redact_value`, which scrub a known *secret
/// value* out of a string wherever it appears -- that works for a tenant
/// secret (the host knows the value up front and never needs to know its
/// location). It does not work for `tenant_key` (PRD-mcphost-session-key
/// requirement 17): the argument can be nested anywhere inside an
/// arbitrary tool's own `args`, and the whole point of the requirement is
/// that redaction must find it by its key name rather than by matching a
/// value the caller controls.
pub fn redact_keys(value: &Value, keys: &[&str]) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    if keys.contains(&k.as_str()) {
                        (k.clone(), Value::String("***".to_string()))
                    } else {
                        (k.clone(), redact_keys(v, keys))
                    }
                })
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(|v| redact_keys(v, keys)).collect()),
        other => other.clone(),
    }
}

#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
    /// The same 32-byte key `cipher` was built from -- kept alongside it so
    /// [`Self::salted_hash`] has a per-deployment salt to hash with,
    /// without a second `$MCPHOST_*` passphrase to configure.
    key_bytes: [u8; 32],
}

impl SecretBox {
    pub fn from_passphrase(passphrase: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(passphrase.as_bytes());
        let key_bytes: [u8; 32] = hasher.finalize().into();
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        Self {
            cipher: Aes256Gcm::new(key),
            key_bytes,
        }
    }

    /// PRD-mcphost-oauth-client-policy Technical considerations: "`ip_hash`
    /// is a salted hash, salt from the existing secrets key, never the raw
    /// address" -- `sha256(key_bytes || value)`, hex-encoded. Deterministic
    /// (same input always hashes the same within one deployment) but not
    /// reversible without the passphrase, and never collides across
    /// deployments with different `$MCPHOST_SECRET_KEY` values.
    pub fn salted_hash(&self, value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.key_bytes);
        hasher.update(value.as_bytes());
        let digest = hasher.finalize();
        let mut s = String::with_capacity(digest.len() * 2);
        for b in digest {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// Encrypt `plaintext`, returning `(ciphertext, nonce)`. A fresh random
    /// 96-bit nonce is generated per call and stored alongside the
    /// ciphertext (AES-GCM nonces must never repeat under the same key).
    pub fn encrypt(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), AppError> {
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| AppError::Internal(format!("secret encryption failed: {e}")))?;
        Ok((ciphertext, nonce_bytes.to_vec()))
    }

    pub fn decrypt(&self, ciphertext: &[u8], nonce: &[u8]) -> Result<String, AppError> {
        if nonce.len() != 12 {
            return Err(AppError::Internal("secret nonce is malformed".into()));
        }
        let nonce = Nonce::from_slice(nonce);
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| AppError::Internal(format!("secret decryption failed: {e}")))?;
        String::from_utf8(plaintext)
            .map_err(|e| AppError::Internal(format!("secret is not valid utf-8: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let sb = SecretBox::from_passphrase("test-passphrase");
        let (ct, nonce) = sb.encrypt("hunter2").unwrap();
        assert_ne!(ct, b"hunter2");
        let pt = sb.decrypt(&ct, &nonce).unwrap();
        assert_eq!(pt, "hunter2");
    }

    #[test]
    fn wrong_key_fails() {
        let sb1 = SecretBox::from_passphrase("key-one");
        let sb2 = SecretBox::from_passphrase("key-two");
        let (ct, nonce) = sb1.encrypt("hunter2").unwrap();
        assert!(sb2.decrypt(&ct, &nonce).is_err());
    }

    #[test]
    fn redact_keys_scrubs_top_level_and_nested_occurrences() {
        let value = serde_json::json!({
            "name": "hello",
            "tenant_key": "top-secret",
            "args": {
                "foo": {"tenant_key": "nested-secret", "other": "kept"},
            },
        });
        let redacted = redact_keys(&value, &["tenant_key"]);
        assert_eq!(redacted["tenant_key"], "***");
        assert_eq!(redacted["args"]["foo"]["tenant_key"], "***");
        assert_eq!(redacted["args"]["foo"]["other"], "kept");
        assert_eq!(redacted["name"], "hello");
        let dump = redacted.to_string();
        assert!(!dump.contains("top-secret"));
        assert!(!dump.contains("nested-secret"));
    }

    #[test]
    fn redact_keys_no_op_when_key_absent() {
        let value = serde_json::json!({"name": "hello", "args": {"msg": "hi"}});
        let redacted = redact_keys(&value, &["tenant_key"]);
        assert_eq!(redacted, value);
    }
}
