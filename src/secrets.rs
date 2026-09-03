//! AES-256-GCM encryption for tenant secret values.
//!
//! `$MCPHOST_SECRET_KEY` is an operator-supplied string of any length; it is
//! SHA-256-hashed to a 32-byte AES-256 key so operators never have to hand
//!-generate exactly 32 bytes.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::errors::AppError;

#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    pub fn from_passphrase(passphrase: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(passphrase.as_bytes());
        let key_bytes = hasher.finalize();
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        Self {
            cipher: Aes256Gcm::new(key),
        }
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
}
