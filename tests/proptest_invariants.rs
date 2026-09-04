//! Property-based invariant tests.
//!
//! Read-only after scaffold. The edit-agent must NOT modify proptests.
//! mcphost-endpoint invariant: `auth::hash_key` (the only form a tenant key
//! is ever stored in, per requirement 2/AC2) is a deterministic 64-hex-char
//! SHA-256 digest that never equals its own plaintext input, for any input
//! the crate might hash.

use mcphost::auth::hash_key;
use proptest::prelude::*;

proptest! {
    #[test]
    fn hash_key_is_deterministic_and_not_identity(key in "\\PC{0,128}") {
        let h1 = hash_key(&key);
        let h2 = hash_key(&key);
        prop_assert_eq!(&h1, &h2, "hash_key must be deterministic");
        prop_assert_eq!(h1.len(), 64, "SHA-256 hex digest is always 64 chars");
        prop_assert!(h1.chars().all(|c| c.is_ascii_hexdigit()), "digest must be lowercase hex");
        prop_assert_ne!(h1, key, "the hash must never equal the plaintext key");
    }
}
