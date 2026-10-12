//! PRD-mcphost-blob-put-get-signed-url R5: the one signed-URL scheme every
//! unauthenticated download route shares (`/exports/{id}`, `/charts/{id}`,
//! `/blobs/{id}`). The message is `"{path}:{expires_unix}"`, HMAC-SHA256,
//! keyed by the owning tenant's `key_hash` -- already stored server-side for
//! every tenant, so this needs no new secret. Rotating the key invalidates
//! every outstanding URL.

use axum::http::StatusCode;

use crate::billing::hmac_sha256;

/// Seconds a freshly minted export, chart or blob URL stays valid.
pub const DEFAULT_TTL_SECS: i64 = 24 * 60 * 60;

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Hex HMAC-SHA256 over `"{path}:{expires_unix}"` keyed by `key_hash`.
pub fn sign(key_hash: &str, path: &str, expires_unix: i64) -> String {
    let message = format!("{path}:{expires_unix}");
    hex_encode(&hmac_sha256(key_hash.as_bytes(), message.as_bytes()))
}

/// The full signed URL: `{public_url}{path}?{expires_param}=<unix>&sig=<hex>`.
/// `expires_param` is the query key the route reads (`expires` for exports
/// and blobs, `exp` for charts -- each kept as its route already shipped).
pub fn mint(public_url: &str, key_hash: &str, path: &str, expires_param: &str, expires_unix: i64) -> String {
    let sig = sign(key_hash, path, expires_unix);
    format!("{public_url}{path}?{expires_param}={expires_unix}&sig={sig}")
}

/// Check a request's `(expires, sig)` query values against the owner's key.
/// `Ok(())` when genuine and fresh; otherwise the status the route answers
/// with: 403 for a missing/malformed/altered signature, 410 for a genuine
/// signature past its expiry.
pub fn verify(
    key_hash: &str,
    path: &str,
    expires_raw: Option<&str>,
    sig: Option<&str>,
    now_unix: i64,
) -> Result<(), (StatusCode, &'static str)> {
    let (Some(expires_raw), Some(sig)) = (expires_raw, sig) else {
        return Err((StatusCode::FORBIDDEN, "missing signature"));
    };
    let Ok(expires_unix) = expires_raw.parse::<i64>() else {
        return Err((StatusCode::FORBIDDEN, "invalid signature"));
    };
    let expected = sign(key_hash, path, expires_unix);
    if !constant_time_eq(sig.as_bytes(), expected.as_bytes()) {
        return Err((StatusCode::FORBIDDEN, "invalid signature"));
    }
    if now_unix > expires_unix {
        return Err((StatusCode::GONE, "link expired"));
    }
    Ok(())
}
