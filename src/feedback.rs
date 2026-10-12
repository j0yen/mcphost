//! PRD-mcphost-feedback-tool-and-ledger: `host.feedback.send` /
//! `host.feedback.list` -- one verb for a stuck agent to tell a human what it
//! wanted, and the `feedback` table behind it.
//!
//! [`FeedbackKind::ALL`] is the single source of the kind vocabulary: the
//! tool schema enum ([`FeedbackKind::schema_enum`]) and the llms.txt line
//! ([`FeedbackKind::llms_txt_kinds`]) are both rendered from it, never typed
//! twice.
//!
//! Call ids: the host keeps no per-connection call list for control-plane
//! verbs, so [`CallRing`] is a 5-slot ring per connection key held in
//! `AppState`; `handler::call_tool` pushes one id per call.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// `text` limit in bytes, after control-character stripping and trim.
pub const TEXT_MAX_BYTES: usize = 2048;
/// Serialized `context` limit in bytes.
pub const CONTEXT_MAX_BYTES: usize = 1024;

/// Anonymous sends allowed per IP per [`CAP_WINDOW_S`].
pub const ANON_SENDS_PER_HOUR: i64 = 5;
/// Keyed-tenant sends allowed per tenant per [`CAP_WINDOW_S`].
pub const TENANT_SENDS_PER_HOUR: i64 = 60;
pub const CAP_WINDOW_S: i64 = 3600;

/// How many most-recent call ids a feedback row records.
pub const CALL_IDS_KEPT: usize = 5;

/// The kind vocabulary. Add a variant here and add it to [`Self::ALL`]; the
/// schema, llms.txt and every consumer follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    MissingTool,
    WrongError,
    DocsGap,
    Bug,
    Praise,
}

impl FeedbackKind {
    /// The one source every consumer reads.
    pub const ALL: [FeedbackKind; 5] = [
        FeedbackKind::MissingTool,
        FeedbackKind::WrongError,
        FeedbackKind::DocsGap,
        FeedbackKind::Bug,
        FeedbackKind::Praise,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FeedbackKind::MissingTool => "missing_tool",
            FeedbackKind::WrongError => "wrong_error",
            FeedbackKind::DocsGap => "docs_gap",
            FeedbackKind::Bug => "bug",
            FeedbackKind::Praise => "praise",
        }
    }

    pub fn parse(s: &str) -> Option<FeedbackKind> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The JSON-schema `enum` array for `host.feedback.send`'s `kind`.
    pub fn schema_enum() -> Value {
        Value::Array(Self::ALL.iter().map(|k| json!(k.as_str())).collect())
    }

    /// The kinds, as the one line `www/llms.txt` carries (backticked,
    /// comma-separated).
    pub fn llms_txt_kinds() -> String {
        Self::ALL
            .iter()
            .map(|k| format!("`{}`", k.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Codes raised through [`crate::errors::AppError::Structured`] by a kind or
/// the handler rather than by a dedicated `AppError` variant, but which the
/// hint table must still cover (PRD-mcphost-error-responses-invite-feedback
/// R1). The drift test builds one `Structured` per entry.
pub const KIND_LEVEL_CODES: &[&str] = &[
    "tool_exception",
    "upstream_status",
    "host_not_allowed",
    "unknown_argument",
    "end_user_required",
    "end_user_explicit_forbidden",
];

/// Every error code the host knows the verb for: the default
/// [`FeedbackKind`] a stuck agent should file under. Adding an `AppError`
/// variant (or a [`KIND_LEVEL_CODES`] entry) without a row here fails
/// `fbeh_every_code_has_a_kind`. A `Structured` code with no row (one-off
/// handler codes) falls back to [`FALLBACK_KIND`].
pub const CODE_TO_KIND: &[(&str, FeedbackKind)] = &[
    ("bearer_invalid", FeedbackKind::DocsGap),
    ("tenant_key_missing", FeedbackKind::DocsGap),
    ("tenant_key_invalid", FeedbackKind::DocsGap),
    ("forbidden", FeedbackKind::WrongError),
    ("tenant_disabled", FeedbackKind::WrongError),
    ("rate_limited", FeedbackKind::WrongError),
    ("tool_not_found", FeedbackKind::MissingTool),
    ("tenant_not_found", FeedbackKind::WrongError),
    ("invalid_params", FeedbackKind::WrongError),
    ("unknown_kind", FeedbackKind::MissingTool),
    ("invalid_tool_name", FeedbackKind::WrongError),
    ("spec_too_large", FeedbackKind::WrongError),
    ("too_many_tools", FeedbackKind::WrongError),
    ("invalid_spec", FeedbackKind::WrongError),
    ("args_invalid", FeedbackKind::WrongError),
    ("secret_missing", FeedbackKind::DocsGap),
    ("call_timeout", FeedbackKind::Bug),
    ("storage", FeedbackKind::Bug),
    ("internal", FeedbackKind::Bug),
    ("registry_disabled", FeedbackKind::WrongError),
    ("namespace_unverified", FeedbackKind::WrongError),
    ("registry_rejected", FeedbackKind::WrongError),
    ("share_quota_exceeded", FeedbackKind::WrongError),
    ("group_not_found", FeedbackKind::WrongError),
    ("handoff_token_invalid", FeedbackKind::WrongError),
    ("handoff_token_redeemed", FeedbackKind::WrongError),
    ("handoff_token_expired", FeedbackKind::WrongError),
    ("issuer_already_registered", FeedbackKind::WrongError),
    ("issuer_quota_exceeded", FeedbackKind::WrongError),
    ("issuer_not_found", FeedbackKind::WrongError),
    ("invalid_token", FeedbackKind::WrongError),
    ("conflicting_credentials", FeedbackKind::WrongError),
    ("wrong_tenant", FeedbackKind::WrongError),
    ("insufficient_scope", FeedbackKind::WrongError),
    ("auth_conflict", FeedbackKind::WrongError),
    ("quota_trusted_issuers", FeedbackKind::WrongError),
    ("tool_exception", FeedbackKind::Bug),
    ("upstream_status", FeedbackKind::Bug),
    ("host_not_allowed", FeedbackKind::WrongError),
    ("unknown_argument", FeedbackKind::WrongError),
    ("end_user_required", FeedbackKind::WrongError),
    ("end_user_explicit_forbidden", FeedbackKind::WrongError),
];

/// The kind for a code with no [`CODE_TO_KIND`] row (an ad-hoc
/// `Structured` code): an unexplained error is a candidate bug.
pub const FALLBACK_KIND: FeedbackKind = FeedbackKind::Bug;

/// The hint's hard size ceiling in bytes.
pub const FEEDBACK_HINT_MAX_BYTES: usize = 200;

/// The kind `code` files under, in `table`.
pub fn kind_in(table: &[(&str, FeedbackKind)], code: &str) -> Option<FeedbackKind> {
    table.iter().find(|(c, _)| *c == code).map(|(_, k)| *k)
}

/// The kind `code` files under in [`CODE_TO_KIND`], else [`FALLBACK_KIND`].
pub fn kind_for_code(code: &str) -> FeedbackKind {
    kind_in(CODE_TO_KIND, code).unwrap_or(FALLBACK_KIND)
}

/// The drift check behind `fbeh_every_code_has_a_kind`: `Err` names the
/// first code with no row in `table`, or the first row no code produces.
pub fn check_every_code_has_a_kind(
    table: &[(&str, FeedbackKind)],
    codes: &[&str],
) -> Result<(), String> {
    for code in codes {
        if kind_in(table, code).is_none() {
            return Err(format!("feedback kind mapping missing for {code}"));
        }
    }
    for (key, _) in table {
        if !codes.contains(key) {
            return Err(format!("feedback kind mapping for {key} matches no error code"));
        }
    }
    Ok(())
}

/// A tool name made safe for the one-line hint: printable, no quote or
/// backslash, at most `max` bytes.
fn hint_safe(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_control() || c == '"' || c == '\\' {
            continue;
        }
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

/// The one-line `data.feedback_hint` for an error `code`, generated from
/// [`CODE_TO_KIND`]. `tool` joins `context` when the error knows it;
/// `request_id` rides in the `text` placeholder so a `bug` report carries
/// the log correlation id. Always <= [`FEEDBACK_HINT_MAX_BYTES`], no newline.
pub fn feedback_hint(code: &str, tool: Option<&str>, request_id: Option<&str>) -> String {
    let kind = kind_for_code(code);
    let text = match request_id {
        Some(id) => format!("<what happened; request_id={}>", hint_safe(id, 32)),
        None => "<what you expected>".to_string(),
    };
    let tool_part = match tool.map(|t| hint_safe(t, 60)) {
        Some(t) if !t.is_empty() => format!(", tool:\"{t}\""),
        _ => String::new(),
    };
    let render = |tool_part: &str| {
        format!(
            "host.feedback.send {{kind:\"{}\", text:\"{}\", context:{{error_code:\"{}\"{}}}}}",
            kind.as_str(),
            text,
            hint_safe(code, 40),
            tool_part
        )
    };
    let hint = render(&tool_part);
    if hint.len() > FEEDBACK_HINT_MAX_BYTES { render("") } else { hint }
}

/// A bounded, in-memory map of connection key -> its most recent call ids.
/// `Arc`-shared across `AppState` clones; never persisted.
#[derive(Clone, Default)]
pub struct CallRing {
    inner: Arc<Mutex<HashMap<String, VecDeque<String>>>>,
}

/// Connection keys held before the oldest are dropped wholesale.
const RING_KEYS_MAX: usize = 10_000;

impl CallRing {
    /// Mints a fresh call id, records it under `key`, returns it.
    pub fn push(&self, key: &str) -> String {
        let id = format!("c_{}", crate::state::new_ulid().to_lowercase());
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if map.len() >= RING_KEYS_MAX && !map.contains_key(key) {
            map.clear();
        }
        let slot = map.entry(key.to_string()).or_default();
        if slot.len() == CALL_IDS_KEPT {
            slot.pop_front();
        }
        slot.push_back(id.clone());
        id
    }

    /// The recorded ids for `key`, oldest first.
    pub fn recent(&self, key: &str) -> Vec<String> {
        let map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.get(key).map(|d| d.iter().cloned().collect()).unwrap_or_default()
    }
}

/// Ring key for an authenticated tenant.
pub fn ring_key_tenant(tenant_id: i64) -> String {
    format!("t:{tenant_id}")
}

/// Ring key for an anonymous connection: its transport session when it has
/// one, else its source address.
pub fn ring_key_anonymous(session_id: Option<&str>, source_ip: &str) -> String {
    match session_id {
        Some(sid) => format!("s:{sid}"),
        None => format!("ip:{source_ip}"),
    }
}

/// Who is sending, as `handler::call_tool` resolved it.
pub struct Sender<'a> {
    pub tenant_id: Option<i64>,
    pub source_ip: &'a str,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub ring_key: String,
}

pub fn tool_descriptors() -> [(&'static str, &'static str); 2] {
    [
        (
            "host.feedback.send",
            "Tell the host's humans what you needed: kind, up to 2048 bytes of text, optional \
             context. Works before signup. Returns feedback_id.",
        ),
        (
            "host.feedback.list",
            "List your own feedback (newest first, status received). Anonymous callers get [].",
        ),
    ]
}

/// `ip_hash`: one-way hash of the source address; never returned anywhere.
fn ip_hash(source_ip: &str) -> String {
    crate::auth::hash_key(&format!("feedback-ip:{source_ip}"))
}

fn new_feedback_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    format!("f_{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// Removes every control character except `\n` and `\t`.
pub fn strip_control_chars(s: &str) -> String {
    s.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect()
}

fn too_long(what: &str, len: usize, max: usize) -> AppError {
    AppError::Structured {
        code: "invalid_params",
        message: format!("{what} must be at most {max} bytes; got {len}"),
        data: json!({"max_bytes": max, "len": len}),
    }
}

/// `host.feedback.send`.
pub async fn send(state: &AppState, sender: &Sender<'_>, args: &Value) -> Result<Value, AppError> {
    let kind_raw = args.get("kind").and_then(Value::as_str).unwrap_or_default();
    let kind = FeedbackKind::parse(kind_raw).ok_or_else(|| {
        AppError::InvalidParams(format!(
            "kind must be one of: {}",
            FeedbackKind::ALL.map(|k| k.as_str()).join(", ")
        ))
    })?;
    let raw_text = args.get("text").and_then(Value::as_str).unwrap_or_default();
    let text = strip_control_chars(raw_text).trim().to_string();
    if text.is_empty() {
        return Err(AppError::InvalidParams("text must not be empty".to_string()));
    }
    if text.len() > TEXT_MAX_BYTES {
        return Err(too_long("text", text.len(), TEXT_MAX_BYTES));
    }
    let context_json = match args.get("context") {
        None | Some(Value::Null) => None,
        Some(c) if c.is_object() => {
            let cleaned = strip_control_chars(&c.to_string());
            if cleaned.len() > CONTEXT_MAX_BYTES {
                return Err(too_long("context", cleaned.len(), CONTEXT_MAX_BYTES));
            }
            Some(cleaned)
        }
        Some(_) => return Err(AppError::InvalidParams("context must be an object".to_string())),
    };
    let call_ids = state.feedback_calls.recent(&sender.ring_key);
    let feedback_id = new_feedback_id();
    // Anonymous senders are capped per IP (fleet IPs exempt, as for signup);
    // keyed tenants per tenant.
    let cap = match sender.tenant_id {
        Some(_) => crate::db::FeedbackCap::Tenant { limit: TENANT_SENDS_PER_HOUR, window_s: CAP_WINDOW_S },
        None if state.fleet_ips.contains(sender.source_ip) => crate::db::FeedbackCap::None,
        None => crate::db::FeedbackCap::Ip { limit: ANON_SENDS_PER_HOUR, window_s: CAP_WINDOW_S },
    };
    let admitted = state
        .db
        .insert_feedback(
            feedback_id.clone(),
            sender.tenant_id,
            kind.as_str().to_string(),
            text,
            context_json,
            sender.client_name.clone(),
            sender.client_version.clone(),
            json!(call_ids).to_string(),
            ip_hash(sender.source_ip),
            now_unix(),
            cap,
        )
        .await?;
    if let Err(retry_after_s) = admitted {
        return Err(AppError::Structured {
            code: "rate_limited",
            message: "too many feedback sends in the last hour".to_string(),
            data: json!({"retry_after_s": retry_after_s}),
        });
    }
    tracing::info!(feedback_id = %feedback_id, kind = kind.as_str(), "feedback received");
    Ok(json!({"feedback_id": feedback_id, "kind": kind.as_str()}))
}

/// Largest `limit` `host.feedback.list` honours.
pub const LIST_LIMIT_MAX: i64 = 100;
const LIST_LIMIT_DEFAULT: i64 = 20;

/// Every row's `status` until a later PRD adds more states.
pub const STATUS_RECEIVED: &str = "received";

/// `host.feedback.list {limit?}`: the caller's own rows, newest first.
/// Anonymous callers get an empty list and a note.
pub async fn list(state: &AppState, sender: &Sender<'_>, args: &Value) -> Result<Value, AppError> {
    let Some(tenant_id) = sender.tenant_id else {
        return Ok(json!({"feedback": [], "count": 0, "note": "sign up to list your feedback"}));
    };
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(LIST_LIMIT_DEFAULT)
        .clamp(1, LIST_LIMIT_MAX);
    let rows = state.db.list_feedback_for_tenant(tenant_id, limit).await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "feedback_id": r.feedback_id,
                "kind": r.kind,
                "text": r.text,
                "context": r.context_json.and_then(|c| serde_json::from_str::<Value>(&c).ok()),
                "last_call_ids": serde_json::from_str::<Value>(&r.last_call_ids).unwrap_or_else(|_| json!([])),
                "status": STATUS_RECEIVED,
                "created_unix": r.created_unix,
            })
        })
        .collect();
    let count = items.len();
    Ok(json!({"feedback": items, "count": count}))
}

/// Parses `--since` as `<n>s|m|h|d` (e.g. `24h`) into seconds.
pub fn parse_since(s: &str) -> Option<i64> {
    let s = s.trim();
    let unit = s.chars().last()?;
    let n: i64 = s[..s.len() - unit.len_utf8()].parse().ok()?;
    let mult = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return None,
    };
    (n >= 0).then(|| n.saturating_mul(mult))
}

/// `mcphost feedback list`'s table: one tab-separated line per row (header
/// first). Carries the tenant namespace, kind, client, call ids and text;
/// `ip_hash` is not part of [`crate::db::FeedbackRow`], so it cannot appear.
pub fn render_table(rows: &[(crate::db::FeedbackRow, Option<String>)]) -> String {
    let mut out = String::from("feedback_id\tcreated_unix\tnamespace\tkind\tclient\tlast_call_ids\ttext\n");
    for (r, ns) in rows {
        let client = match (&r.client_name, &r.client_version) {
            (Some(n), Some(v)) => format!("{n}/{v}"),
            (Some(n), None) => n.clone(),
            _ => "-".to_string(),
        };
        let ids: Vec<String> = serde_json::from_str(&r.last_call_ids).unwrap_or_default();
        let text = r.text.replace(['\n', '\t'], " ");
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            r.feedback_id,
            r.created_unix,
            ns.as_deref().unwrap_or("-"),
            r.kind,
            client,
            if ids.is_empty() { "-".to_string() } else { ids.join(",") },
            text
        ));
    }
    out
}
