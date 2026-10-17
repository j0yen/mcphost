//! PRD-mcphost-idempotency-key-on-call: a `tools/call` may carry an
//! idempotency key; a repeat with the same key (and args) returns the stored
//! result instead of running the tool twice.
//!
//! Every wire string (code name, both key positions, TTL, hint) is a `const`
//! here so the `tool_timeout` error, the docs and the tests read one source.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::errors::AppError;

/// The `_meta` key a client passes the idempotency key under.
pub const META_KEY: &str = "idempotency_key";
/// The `args` key for clients that drop `_meta`; stripped before validation.
pub const ARG_KEY: &str = "__idempotency_key";
/// R6: the hint a `tool_timeout` error carries.
pub const TIMEOUT_HINT: &str = "retry with the same _meta.idempotency_key to get the stored result";
/// R4: the wire code for the same key with different args.
pub const CONFLICT_CODE: &str = "idempotency_conflict";
/// R4: the hint a conflict carries.
pub const CONFLICT_HINT: &str = "use a new key for a different request";
/// R4: the error for the same key arriving with different args.
pub fn conflict_error(key: &str, stored_args_sha256: &str, given_args_sha256: &str) -> AppError {
    AppError::Structured {
        code: CONFLICT_CODE,
        message: "this idempotency key was already used with different arguments".to_string(),
        data: json!({
            "key": key,
            "stored_args_sha256": stored_args_sha256,
            "given_args_sha256": given_args_sha256,
            "hint": CONFLICT_HINT,
        }),
    }
}

/// R2: how long a stored result is kept.
pub const TTL_SECS: i64 = 24 * 60 * 60;
/// R1: the longest accepted key.
pub const MAX_KEY_LEN: usize = 128;

/// R1: read the key from `_meta.idempotency_key`, else `args.__idempotency_key`
/// (always removed from `args`). `Ok(None)` when no key was sent.
pub fn extract_key(meta: Option<&Map<String, Value>>, args: &mut Value) -> Result<Option<String>, AppError> {
    let from_args = match args {
        Value::Object(map) => map.remove(ARG_KEY),
        _ => None,
    };
    let raw = meta.and_then(|m| m.get(META_KEY)).cloned().or(from_args);
    let Some(raw) = raw else { return Ok(None) };
    let key = raw.as_str().unwrap_or_default();
    let valid = !key.is_empty()
        && key.len() <= MAX_KEY_LEN
        && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'));
    if !valid {
        return Err(AppError::Structured {
            code: crate::errors::INVALID_PARAMS,
            message: format!(
                "idempotency_key must be 1..={MAX_KEY_LEN} characters from [A-Za-z0-9_.:-]"
            ),
            data: json!({"argument": "idempotency_key"}),
        });
    }
    Ok(Some(key.to_string()))
}

fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// R2: sha256 (hex) over the canonical (sorted-key) JSON of `args`.
pub fn args_sha256(args: &Value) -> String {
    let mut s = String::new();
    canonical(args, &mut s);
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// R6: a `tool_timeout` error gains `hint` and `idempotency_key` (the key
/// the call carried, or null) so an agent learns the move when it needs it.
pub fn annotate_timeout(err: AppError, key: Option<&str>) -> AppError {
    match err {
        AppError::Structured { code: TOOL_TIMEOUT, message, data } => {
            let mut map = match data {
                Value::Object(m) => m,
                Value::Null => Map::new(),
                other => Map::from_iter([("detail".to_string(), other)]),
            };
            map.insert("hint".to_string(), json!(TIMEOUT_HINT));
            map.insert("idempotency_key".to_string(), json!(key));
            AppError::Structured { code: TOOL_TIMEOUT, message, data: Value::Object(map) }
        }
        other => other,
    }
}

/// R5: the longest a second caller waits on a running first call; a claim
/// older than this is treated as abandoned and taken over.
pub const WAIT_MAX_SECS: i64 = 30;

/// A finished call's stored outcome.
#[derive(Debug, Clone)]
pub struct Stored {
    pub outcome: Result<Value, AppError>,
    pub first_seen_unix: i64,
}

/// What [`Db::idem_claim`] found.
#[derive(Debug)]
pub enum Claim {
    /// No live row: this caller owns the key and must run the tool.
    Run,
    /// A finished row with the same args hash.
    Replay(Stored),
    /// A row exists for the key with a different args hash.
    Conflict { stored_args_sha256: String },
    /// The first call is still running.
    Pending,
}

/// Error codes whose outcome is deterministic and worth replaying. Anything
/// else (a `tool_timeout`, storage or auth failure) is transient: the claim
/// is released so a retry executes once.
fn storable(err: &AppError) -> bool {
    match err {
        AppError::Structured { code, .. } => *code != "tool_timeout" && *code != "idempotency_conflict",
        AppError::InvalidArgs(_) => true,
        _ => false,
    }
}

fn error_to_json(err: &AppError) -> Value {
    let data = match err {
        AppError::Structured { data, .. } => data.clone(),
        _ => Value::Null,
    };
    json!({"code": err.code(), "message": err.to_string(), "data": data})
}

/// Codes seen on replay, resolved to the registry's own `&'static str`
/// so `Structured { code: &'static str }` can carry them. Every code a
/// replay row can hold was minted by [`AppError::code`] or by a kind's
/// contract family, so the lookup is closed; a code outside both reads
/// `internal` (the row is still an error, just not a classified one).
fn intern(code: &str) -> &'static str {
    if let Some(found) = crate::kinds::contract::all_families()
        .iter()
        .flat_map(|f| f.codes.iter().copied())
        .find(|c| *c == code)
    {
        return found;
    }
    REPLAY_CODES.iter().copied().find(|c| *c == code).unwrap_or("internal")
}

/// The fixed codes [`AppError::code`] returns outside the kind families
/// (its literal match arms), so a replayed row round-trips its own code.
const REPLAY_CODES: &[&str] = &[
    "args_invalid",
    "handoff_token_invalid",
    "handoff_token_redeemed",
    "idempotency_conflict",
    "insufficient_scope",
    "internal",
    "invalid_params",
    "invalid_spec",
    "invalid_token",
    "invalid_tool_name",
    "issuer_already_registered",
    "issuer_not_found",
    "issuer_quota_exceeded",
    "namespace_unverified",
    "quota_trusted_issuers",
    "rate_limited",
    "registry_disabled",
    "registry_rejected",
    "secret_missing",
    "share_quota_exceeded",
    "spec_too_large",
    "storage",
    "tenant_disabled",
    "tenant_key_invalid",
    "tenant_key_missing",
    "tenant_not_found",
    "too_many_tools",
    "tool_not_found",
    "unknown_kind",
    "wrong_tenant",
];

crate::err_code! {
    pub TOOL_TIMEOUT,
    code: "tool_timeout", family: Call, class: AgentFault, retryable: false,
    remedy: "Make the tool finish within the time cap, or reduce its input.",
}


#[cfg(test)]
mod intern_tests {
    use super::intern;

    #[test]
    fn replayed_codes_round_trip_and_unknown_reads_internal() {
        assert_eq!(intern("tool_timeout"), "tool_timeout");
        assert_eq!(intern("invalid_params"), "invalid_params");
        assert_eq!(intern("idempotency_conflict"), "idempotency_conflict");
        assert_eq!(intern("no_such_code_ever"), "internal");
    }
}

fn error_from_json(v: &Value) -> AppError {
    let code = v["code"].as_str().unwrap_or("internal");
    AppError::Structured {
        code: intern(code),
        message: v["message"].as_str().unwrap_or_default().to_string(),
        data: v["data"].clone(),
    }
}

impl crate::db::Db {
    /// R2/R3/R4/R5: one transaction-free atomic step. Drops this key's
    /// expired (or abandoned-pending) row, then INSERT-OR-IGNOREs a pending
    /// claim; losing the insert means a live row exists and is classified.
    pub async fn idem_claim(
        &self,
        tenant_id: i64,
        tool: &str,
        key: &str,
        args_sha256: &str,
    ) -> Result<Claim, AppError> {
        let now = crate::state::now_unix();
        let (tool, key, sha) = (tool.to_string(), key.to_string(), args_sha256.to_string());
        self.with_conn(move |conn| {
            use rusqlite::params;
            conn.execute(
                "DELETE FROM call_results WHERE tenant_id = ?1 AND tool_name = ?2 AND key = ?3 \
                 AND (expires_unix <= ?4 OR (result_json IS NULL AND error_json IS NULL AND created_unix <= ?5))",
                params![tenant_id, tool, key, now, now - WAIT_MAX_SECS],
            )?;
            let inserted = conn.execute(
                "INSERT OR IGNORE INTO call_results \
                 (tenant_id, tool_name, key, args_sha256, created_unix, expires_unix) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![tenant_id, tool, key, sha, now, now + TTL_SECS],
            )?;
            if inserted == 1 {
                return Ok(Claim::Run);
            }
            let (stored_sha, result, error, created): (String, Option<String>, Option<String>, i64) = conn
                .query_row(
                    "SELECT args_sha256, result_json, error_json, created_unix FROM call_results \
                     WHERE tenant_id = ?1 AND tool_name = ?2 AND key = ?3",
                    params![tenant_id, tool, key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?;
            if stored_sha != sha {
                return Ok(Claim::Conflict { stored_args_sha256: stored_sha });
            }
            let outcome = match (result, error) {
                (Some(r), _) => Ok(serde_json::from_str(&r).unwrap_or(Value::Null)),
                (None, Some(e)) => Err(error_from_json(&serde_json::from_str(&e).unwrap_or(Value::Null))),
                (None, None) => return Ok(Claim::Pending),
            };
            Ok(Claim::Replay(Stored { outcome, first_seen_unix: created }))
        })
        .await
    }

    /// Fill the claim with the call's outcome, or release it (a transient
    /// outcome is never stored, so a retry executes once).
    pub async fn idem_finish(
        &self,
        tenant_id: i64,
        tool: &str,
        key: &str,
        outcome: &Result<Value, AppError>,
    ) -> Result<(), AppError> {
        let (tool, key) = (tool.to_string(), key.to_string());
        let (result, error) = match outcome {
            Ok(v) => (Some(v.to_string()), None),
            Err(e) if storable(e) => (None, Some(error_to_json(e).to_string())),
            Err(_) => (None, None),
        };
        self.with_conn(move |conn| {
            use rusqlite::params;
            if result.is_none() && error.is_none() {
                conn.execute(
                    "DELETE FROM call_results WHERE tenant_id = ?1 AND tool_name = ?2 AND key = ?3",
                    params![tenant_id, tool, key],
                )?;
            } else {
                conn.execute(
                    "UPDATE call_results SET result_json = ?4, error_json = ?5 \
                     WHERE tenant_id = ?1 AND tool_name = ?2 AND key = ?3",
                    params![tenant_id, tool, key, result, error],
                )?;
            }
            Ok(())
        })
        .await
    }

    /// R2: nightly purge of expired rows.
    pub async fn idem_purge_expired(&self, now: i64) -> Result<usize, AppError> {
        self.with_conn(move |conn| {
            Ok(conn.execute("DELETE FROM call_results WHERE expires_unix <= ?1", rusqlite::params![now])?)
        })
        .await
    }
}

/// Releases a still-pending claim if the call path exits without finishing
/// (an early `return`, a dropped request future), so a retry never waits on
/// a claim nobody will fill.
pub struct ClaimGuard {
    db: crate::db::Db,
    tenant_id: i64,
    tool: String,
    key: String,
    done: bool,
}

impl ClaimGuard {
    pub fn new(db: crate::db::Db, tenant_id: i64, tool: &str, key: &str) -> Self {
        Self { db, tenant_id, tool: tool.to_string(), key: key.to_string(), done: false }
    }

    pub async fn finish(mut self, outcome: &Result<Value, AppError>) {
        self.done = true;
        if let Err(e) = self.db.idem_finish(self.tenant_id, &self.tool, &self.key, outcome).await {
            tracing::warn!(error = %e, tool = %self.tool, "failed to store idempotent call result");
        }
    }
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let (db, tenant_id, tool, key) = (self.db.clone(), self.tenant_id, self.tool.clone(), self.key.clone());
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = db.idem_finish(tenant_id, &tool, &key, &Err(AppError::CallTimeout(0))).await;
            });
        }
    }
}
