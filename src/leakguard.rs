//! PRD-mcphost-secret-non-exfiltration-proof: a secret that tries to leave
//! is blocked, not just redacted.
//!
//! Three questions live here so `kinds::http` (and the python result path)
//! cannot answer them differently: does a resolved secret value appear in
//! some text ([`contains_secret`]), is a request host one a secret may be
//! sent to ([`host_allowed`]), and what does a block look like on the wire
//! ([`blocked`]).

use serde_json::{Value, json};

use crate::kinds::KindError;

/// A secret value of at least this many characters is matched as a
/// substring; a shorter one only as a whole token (so a 3-character secret
/// does not block every result containing those letters inside a word).
pub const MIN_SUBSTRING_LEN: usize = 8;

/// The reserved hostname of the in-process echo upstream
/// `host.secret.leak_test` uses when the caller names no echo URL; it never
/// leaves the process and never resolves (RFC 2606).
pub const LEAK_ECHO_HOST: &str = "leak-echo.invalid";

/// The host the self-test's egress leg targets; never resolves (RFC 2606),
/// so a `LEAKED` verdict can only mean the outbound block was missing.
pub const LEAK_CANARY_HOST: &str = "leak-canary.invalid";

/// Prefix of the transient fixture tools `host.secret.leak_test` publishes.
pub const FIXTURE_PREFIX: &str = "__leak_test_";

crate::err_code! {
    pub SECRET_LEAK_BLOCKED,
    code: "secret_leak_blocked", family: Call, class: AgentFault, retryable: false,
    remedy: "Stop the tool from putting the secret in data.where (url, header, body, result, excerpt or log), or bind the secret to the right host with host.secret.set hosts.",
}

crate::err_code! {
    pub LEAK_TEST_FAILED,
    code: "leak_test_failed", family: Call, class: HostFault, retryable: false,
    remedy: "A secret got through the self-test: the host's leak block is not working; report data.echo and data.egress to the operator.",
}

fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Does `value` appear in `hay`? Substring for a value of
/// [`MIN_SUBSTRING_LEN`]+ characters, whole token (neighbours are not
/// alphanumeric or `_`) for a shorter one. An empty value never matches.
pub fn contains_secret(hay: &str, value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if value.chars().count() >= MIN_SUBSTRING_LEN {
        return hay.contains(value);
    }
    hay.match_indices(value).any(|(i, m)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + m.len()..].chars().next();
        !before.is_some_and(is_token_char) && !after.is_some_and(is_token_char)
    })
}

/// `hay` with every occurrence [`contains_secret`] would match replaced by
/// `***` -- so a short secret inside a longer word is left alone, exactly
/// as it is not a block.
pub fn redact_secret(hay: &str, value: &str) -> String {
    if value.is_empty() {
        return hay.to_string();
    }
    if value.chars().count() >= MIN_SUBSTRING_LEN {
        return hay.replace(value, "***");
    }
    let mut out = String::with_capacity(hay.len());
    let mut last = 0;
    for (i, m) in hay.match_indices(value) {
        let before = hay[..i].chars().next_back();
        let after = hay[i + m.len()..].chars().next();
        if !before.is_some_and(is_token_char) && !after.is_some_and(is_token_char) {
            out.push_str(&hay[last..i]);
            out.push_str("***");
            last = i + m.len();
        }
    }
    out.push_str(&hay[last..]);
    out
}

/// Does any string in `value` (keys and leaves, any depth) carry `secret`?
pub fn value_contains_secret(value: &Value, secret: &str) -> bool {
    match value {
        Value::String(s) => contains_secret(s, secret),
        Value::Array(a) => a.iter().any(|v| value_contains_secret(v, secret)),
        Value::Object(m) => m
            .iter()
            .any(|(k, v)| contains_secret(k, secret) || value_contains_secret(v, secret)),
        _ => false,
    }
}

/// Is `host` one of `bound`? Exact (case-insensitive) match, or a
/// `*.suffix` entry matching any subdomain of `suffix`.
pub fn host_allowed(bound: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    bound.iter().any(|b| {
        let b = b.to_ascii_lowercase();
        match b.strip_prefix("*.") {
            Some(suffix) => host.ends_with(&format!(".{suffix}")),
            None => b == host,
        }
    })
}

/// Normalise a `hosts` entry from `host.secret.set`: a bare lower-case
/// hostname (or `*.suffix`), no scheme, port or path.
pub fn normalize_host(raw: &str) -> Option<String> {
    let h = raw.trim().to_ascii_lowercase();
    let bare = h.strip_prefix("*.").unwrap_or(&h);
    let ok = !bare.is_empty()
        && bare.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    ok.then_some(h)
}

tokio::task_local! {
    /// Names of unbound secrets the current `tools/call` sent upstream;
    /// scoped by `handler::call_tool`, read back into `_meta`.
    pub static UNBOUND_SENT: std::cell::RefCell<std::collections::BTreeSet<String>>;
}

/// Record that the unbound secret `name` was sent out on this call.
pub fn note_unbound_sent(name: &str) {
    let _ = UNBOUND_SENT.try_with(|set| set.borrow_mut().insert(name.to_string()));
}

/// The names [`note_unbound_sent`] collected on this call, sorted.
pub fn take_unbound_sent() -> Vec<String> {
    UNBOUND_SENT
        .try_with(|set| std::mem::take(&mut *set.borrow_mut()).into_iter().collect())
        .unwrap_or_default()
}

/// The `admin_audit` action a block is recorded under.
pub const AUDIT_ACTION: &str = "secret_leak_blocked";

/// Append the audit row for a block (PRD-mcphost-secret-non-exfiltration-proof
/// R5): `{tenant, tool, where, secret}`. Best effort -- a failed write never
/// changes the call's outcome.
pub async fn record_block(db: &crate::db::Db, tenant: &str, tool: &str, data: &Value) {
    let detail = json!({
        "tenant": tenant,
        "tool": tool,
        "where": data.get("where").cloned().unwrap_or(Value::Null),
        "secret": data.get("secret").cloned().unwrap_or(Value::Null),
    });
    let _ = db
        .record_admin_audit(
            "host".to_string(),
            AUDIT_ACTION.to_string(),
            Some(format!("{tenant}.{tool}")),
            Some(detail.to_string()),
        )
        .await;
}

/// The error for a blocked leak. `extra` is merged into `data` next to
/// `where` and `secret`.
pub fn blocked(where_: &str, secret: &str, extra: Value) -> KindError {
    let mut data = json!({"where": where_, "secret": secret});
    if let (Some(obj), Value::Object(more)) = (data.as_object_mut(), extra) {
        obj.extend(more);
    }
    KindError::structured_with(
        SECRET_LEAK_BLOCKED,
        format!("secret '{secret}' would leave in the {where_}; the call was blocked"),
        data,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_values_match_as_substrings() {
        assert!(contains_secret("xxsk_live_abcdefxx", "sk_live_abcdef"));
    }

    #[test]
    fn short_values_match_whole_tokens_only() {
        assert!(contains_secret("token=abc&x=1", "abc"));
        assert!(contains_secret("abc", "abc"));
        assert!(!contains_secret("abcdef and xabc", "abc"));
    }

    #[test]
    fn short_values_are_redacted_as_whole_tokens_only() {
        assert_eq!(redact_secret("token abc and abcdef", "abc"), "token *** and abcdef");
        assert_eq!(redact_secret("xx sk_live_abcdef yy", "sk_live_abcdef"), "xx *** yy");
    }

    #[test]
    fn bound_hosts_match_exact_and_wildcard() {
        let bound = vec!["api.example.com".to_string(), "*.corp.test".to_string()];
        assert!(host_allowed(&bound, "API.example.com"));
        assert!(host_allowed(&bound, "a.corp.test"));
        assert!(!host_allowed(&bound, "corp.test"));
        assert!(!host_allowed(&bound, "evil.example"));
    }
}
