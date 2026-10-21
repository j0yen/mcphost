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

use rusqlite::{OptionalExtension, params};
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
    (crate::scopes::INSUFFICIENT_SCOPE, FeedbackKind::WrongError),
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

pub fn tool_descriptors() -> [(&'static str, &'static str); 3] {
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
        (
            "host.feedback.get",
            "Read one of your own feedback rows by feedback_id, with the theme_id and theme \
             status it was clustered into (null theme_id until the nightly pass reaches it). \
             Anyone else's id is feedback_not_found.",
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
        code: crate::errors::INVALID_PARAMS,
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
            code: crate::errors::RATE_LIMITED,
            message: "too many feedback sends in the last hour".to_string(),
            data: json!({"retry_after_s": retry_after_s}),
        });
    }
    tracing::info!(feedback_id = %feedback_id, kind = kind.as_str(), "feedback received");
    Ok(json!({"feedback_id": feedback_id, "kind": kind.as_str()}))
}

/// `host.feedback.get {feedback_id}` (PRD-mcphost-feedback-themes-nightly
/// R6): the caller's own row, with its theme. A row the caller does not own
/// -- and every row, for an anonymous caller -- is `feedback_not_found`, the
/// same answer as an id that never existed.
pub async fn get(state: &AppState, sender: &Sender<'_>, args: &Value) -> Result<Value, AppError> {
    let id = args.get("feedback_id").and_then(Value::as_str).unwrap_or_default().trim().to_string();
    if id.is_empty() {
        return Err(AppError::InvalidParams("feedback_id must not be empty".to_string()));
    }
    let not_found = || AppError::Structured {
        code: FEEDBACK_NOT_FOUND,
        message: format!("no feedback '{id}' filed by this caller"),
        data: json!({"feedback_id": id}),
    };
    let Some(tenant_id) = sender.tenant_id else { return Err(not_found()) };
    let row = state.db.get_feedback(id.clone()).await?;
    let Some(row) = row.filter(|r| r.tenant_id == Some(tenant_id)) else {
        return Err(not_found());
    };
    let theme_id = state.db.feedback_row_theme_id(row.feedback_id.clone()).await?;
    let theme = match &theme_id {
        Some(t) => state.db.get_feedback_theme(t.clone()).await?,
        None => None,
    };
    Ok(json!({
        "feedback_id": row.feedback_id,
        "kind": row.kind,
        "text": row.text,
        "context": row.context_json.and_then(|c| serde_json::from_str::<Value>(&c).ok()),
        "last_call_ids": serde_json::from_str::<Value>(&row.last_call_ids).unwrap_or_else(|_| json!([])),
        "theme_id": theme_id,
        "status": theme.map_or(STATUS_RECEIVED.to_string(), |t| t.status),
        "created_unix": row.created_unix,
    }))
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

// ---------------------------------------------------------------------------
// PRD-mcphost-feedback-themes-nightly: LLM-free theme key.
// ---------------------------------------------------------------------------

/// Words that carry no ask (R2: a 40-word list). Reporting verbs are here so
/// `said` / `says` / `reports` do not split one complaint into several.
pub const STOP_WORDS: [&str; 40] = [
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "it", "its", "this", "that",
    "to", "of", "in", "on", "at", "for", "with", "and", "or", "but", "so", "i", "my", "me",
    "we", "you", "your", "said", "says", "say", "saying", "reported", "reports", "got", "get",
    "gets", "getting",
];

/// Characters kept in an alphabetic token: letters and `_` (so an error code
/// such as `args_invalid` stays one word).
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// True for `f_…` / `t_…` / `th_…` / `c_…` ids (hex-ish tail).
fn is_prefixed_id(tok: &str) -> bool {
    ["th_", "f_", "t_", "c_"].iter().any(|p| {
        tok.strip_prefix(p)
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()))
    })
}

/// Replaces quoted strings (`"…"`, `` `…` ``, and `'…'` that opens after a
/// non-alphanumeric so contractions survive) with a single `_` token.
fn blank_quoted(lower: &str) -> String {
    let chars: Vec<char> = lower.chars().collect();
    let mut out = String::with_capacity(lower.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let opens = match c {
            '"' | '`' => true,
            '\'' => i == 0 || !chars[i - 1].is_alphanumeric(),
            _ => false,
        };
        if opens {
            // A backslash escapes the next char, so `"a \"b\" c"` (a JSON body
            // echoed into the error) is one quoted value, not three.
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            if j < chars.len() {
                out.push_str(" _ ");
                i = j + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Drops a trailing ` at <file>.py:<line>` traceback location: the line number
/// differs per tenant tool while the error is the same ask.
fn strip_traceback_location(lower: &str) -> String {
    if let Some((head, tail)) = lower.rsplit_once(" at ")
        && let Some((file, line)) = tail.trim().rsplit_once(':')
        && file.ends_with(".py")
        && !line.is_empty()
        && line.chars().all(|c| c.is_ascii_digit())
    {
        return head.to_string();
    }
    lower.to_string()
}

/// R2: the normalized prefix of `text`. Lowercases, replaces quoted strings,
/// digit runs, hex ids of 8+ chars, dotted paths and `f_`/`t_`/`th_` ids with `_`, drops
/// [`STOP_WORDS`], and keeps the first 6 alphabetic tokens (a run of `_`
/// placeholders between them is kept as one `_`). Pure.
pub fn normalize(text: &str) -> String {
    let lower = blank_quoted(&strip_traceback_location(&text.to_lowercase()));
    let mut parts: Vec<String> = Vec::new();
    let mut words = 0usize;
    let mut cur = String::new();
    let flush = |cur: &mut String, parts: &mut Vec<String>, words: &mut usize| {
        if cur.is_empty() {
            return;
        }
        let tok = std::mem::take(cur);
        let all_hex = tok.len() >= 8 && tok.chars().all(|c| c.is_ascii_hexdigit());
        let placeholder = tok == "_"
            || tok == "null"
            || is_prefixed_id(&tok)
            || all_hex
            || tok.contains('.')
            || tok.chars().any(|c| c.is_ascii_digit() || !c.is_ascii());
        if placeholder {
            if parts.last().map(String::as_str) != Some("_") {
                parts.push("_".to_string());
            }
        } else if tok.chars().all(is_word_char)
            && !tok.chars().all(|c| c == '_')
            && !STOP_WORDS.contains(&tok.as_str())
            && *words < 6
        {
            parts.push(tok);
            *words += 1;
        }
    };
    let chars: Vec<char> = lower.chars().collect();
    let joins = |c: char| c.is_ascii_alphanumeric() || c == '_';
    for (i, &c) in chars.iter().enumerate() {
        // A `.` between word chars joins a dotted path (`budget.max_tool_latency_ms`,
        // `t_6238451c.lc_chain_runner`), which is a value and becomes `_`.
        let dotted = c == '.' && i > 0 && joins(chars[i - 1]) && chars.get(i + 1).is_some_and(|n| joins(*n));
        if c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii() || dotted {
            cur.push(c);
        } else {
            flush(&mut cur, &mut parts, &mut words);
        }
    }
    flush(&mut cur, &mut parts, &mut words);
    while parts.last().map(String::as_str) == Some("_") {
        parts.pop();
    }
    parts.join(" ")
}

/// The theme key's four parts: `kind|error_code|tool|prefix`.
pub fn theme_key(kind: &str, error_code: &str, tool: &str, text: &str) -> String {
    format!("{kind}|{error_code}|{}|{}", normalize_tool(tool), normalize(text))
}

/// The tool part of a theme key without per-tenant instance noise: tenant tool
/// names carry an 8-hex build suffix or prefix (`lambda_cost_watch_358f31f4`,
/// `dc617cce_transform`) that differs per run for the same ask. Pure.
pub fn normalize_tool(tool: &str) -> String {
    let is_hex8 = |s: &str| s.len() == 8 && s.chars().all(|c| c.is_ascii_hexdigit());
    let mut t = tool;
    if let Some((head, tail)) = t.rsplit_once('_') && is_hex8(tail) {
        t = head;
    }
    if let Some((head, tail)) = t.split_once('_') && is_hex8(head) {
        t = tail;
    }
    t.to_string()
}

/// `th_` + 12 hex of `sha256(key)[..6]`: stable across runs and hosts.
pub fn theme_id_for_key(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    format!("th_{}", digest[..6].iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// The `(error_code, tool)` a feedback row's `context_json` carries (empty
/// strings when absent).
pub fn context_code_tool(context_json: Option<&str>) -> (String, String) {
    let v: Value = context_json.and_then(|c| serde_json::from_str(c).ok()).unwrap_or(Value::Null);
    let get = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    (get("error_code"), get("tool"))
}

/// The theme id a whole row maps to.
pub fn theme_id_for_row(kind: &str, context_json: Option<&str>, text: &str) -> String {
    let (code, tool) = context_code_tool(context_json);
    theme_id_for_key(&theme_key(kind, &code, &tool, text))
}

// ---------------------------------------------------------------------------
// Theme table access.
// ---------------------------------------------------------------------------

/// Caps from R1 / Technical considerations.
pub const THEME_EXAMPLES_MAX: usize = 5;
pub const THEME_TENANTS_MAX: usize = 20;
pub const THEME_CALL_IDS_MAX: usize = 25;
/// Largest theme list `admin.feedback.themes` returns.
pub const THEMES_LIMIT_MAX: i64 = 500;
pub const THEME_STATUS_OPEN: &str = "open";

/// One `feedback_themes` row.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackTheme {
    pub theme_id: String,
    pub kind: String,
    pub error_code: String,
    pub tool: String,
    pub text_prefix: String,
    pub count: i64,
    pub tenants: Vec<String>,
    pub example_ids: Vec<String>,
    pub call_ids: Vec<String>,
    pub first_seen_unix: i64,
    pub last_seen_unix: i64,
    pub status: String,
}

impl FeedbackTheme {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let arr = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
        Ok(Self {
            theme_id: r.get(0)?,
            kind: r.get(1)?,
            error_code: r.get(2)?,
            tool: r.get(3)?,
            text_prefix: r.get(4)?,
            count: r.get(5)?,
            tenants: arr(r.get(6)?),
            example_ids: arr(r.get(7)?),
            call_ids: arr(r.get(8)?),
            first_seen_unix: r.get(9)?,
            last_seen_unix: r.get(10)?,
            status: r.get(11)?,
        })
    }

    /// The wire shape `admin.feedback.themes` and the CLI print.
    pub fn to_json(&self) -> Value {
        json!({
            "theme_id": self.theme_id,
            "kind": self.kind,
            "error_code": self.error_code,
            "tool": self.tool,
            "text_prefix": self.text_prefix,
            "count": self.count,
            "tenants": self.tenants,
            "example_ids": self.example_ids,
            "last_call_ids": self.call_ids,
            "first_seen_unix": self.first_seen_unix,
            "last_seen_unix": self.last_seen_unix,
            "status": self.status,
        })
    }
}

const THEME_COLS: &str = "theme_id, kind, error_code, tool, text_prefix, count, tenants_json, \
     example_ids_json, call_ids_json, first_seen_unix, last_seen_unix, status";

/// What one clustering pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClusterStats {
    /// `feedback` rows whose `theme_id` was set this pass.
    pub rows_clustered: usize,
    /// Distinct themes created or grown.
    pub themes_touched: usize,
}

/// `(id, feedback_id, kind, text, context_json, tenant_id, last_call_ids, created_unix)`.
type PendingRow = (i64, String, String, String, Option<String>, Option<i64>, String, i64);

fn push_capped(v: &mut Vec<String>, item: &str, cap: usize) {
    if v.len() < cap && !v.iter().any(|x| x == item) {
        v.push(item.to_string());
    }
}

impl crate::db::Db {
    /// R3: for each `feedback` row with a NULL `theme_id` (oldest first),
    /// computes the key, upserts the theme and stamps the row. Idempotent: a
    /// pass with no unclustered rows writes nothing at all.
    pub async fn cluster_feedback(&self) -> Result<ClusterStats, AppError> {
        self.with_conn(|conn| {
            let pending: Vec<PendingRow> = {
                let mut stmt = conn.prepare(
                    "SELECT id, feedback_id, kind, text, context_json, tenant_id, last_call_ids, \
                     created_unix FROM feedback WHERE theme_id IS NULL \
                     ORDER BY created_unix, id",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
                })?;
                rows.collect::<Result<Vec<_>, _>>()?
            };
            if pending.is_empty() {
                return Ok(ClusterStats::default());
            }
            crate::txn::with_tx(conn, crate::txn::TxBehavior::Deferred, |tx| {
                let mut touched: HashMap<String, FeedbackTheme> = HashMap::new();
                for (id, feedback_id, kind, text, context_json, tenant_id, call_ids, created) in &pending {
                    let (code, tool) = context_code_tool(context_json.as_deref());
                    let prefix = normalize(text);
                    let theme_id = theme_id_for_key(&format!("{kind}|{code}|{tool}|{prefix}"));
                    if !touched.contains_key(&theme_id) {
                        let existing = tx
                            .query_row(
                                &format!("SELECT {THEME_COLS} FROM feedback_themes WHERE theme_id = ?1"),
                                params![theme_id],
                                FeedbackTheme::from_row,
                            )
                            .optional()?;
                        let theme = existing.unwrap_or_else(|| FeedbackTheme {
                            theme_id: theme_id.clone(),
                            kind: kind.clone(),
                            error_code: code.clone(),
                            tool: tool.clone(),
                            text_prefix: prefix.clone(),
                            count: 0,
                            tenants: Vec::new(),
                            example_ids: Vec::new(),
                            call_ids: Vec::new(),
                            first_seen_unix: *created,
                            last_seen_unix: *created,
                            status: THEME_STATUS_OPEN.to_string(),
                        });
                        touched.insert(theme_id.clone(), theme);
                    }
                    let theme = touched.get_mut(&theme_id).expect("inserted above");
                    theme.count += 1;
                    theme.first_seen_unix = theme.first_seen_unix.min(*created);
                    theme.last_seen_unix = theme.last_seen_unix.max(*created);
                    push_capped(&mut theme.example_ids, feedback_id, THEME_EXAMPLES_MAX);
                    if let Some(t) = tenant_id {
                        push_capped(&mut theme.tenants, &format!("t{t}"), THEME_TENANTS_MAX);
                    }
                    for c in serde_json::from_str::<Vec<String>>(call_ids).unwrap_or_default() {
                        push_capped(&mut theme.call_ids, &c, THEME_CALL_IDS_MAX);
                    }
                    tx.execute("UPDATE feedback SET theme_id = ?1 WHERE id = ?2", params![theme_id, id])?;
                }
                for t in touched.values() {
                    tx.execute(
                        "INSERT INTO feedback_themes (theme_id, kind, error_code, tool, text_prefix, count, \
                         tenants_json, example_ids_json, call_ids_json, first_seen_unix, last_seen_unix, status) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
                         ON CONFLICT(theme_id) DO UPDATE SET count = excluded.count, \
                         tenants_json = excluded.tenants_json, example_ids_json = excluded.example_ids_json, \
                         call_ids_json = excluded.call_ids_json, first_seen_unix = excluded.first_seen_unix, \
                         last_seen_unix = excluded.last_seen_unix",
                        params![
                            t.theme_id, t.kind, t.error_code, t.tool, t.text_prefix, t.count,
                            json!(t.tenants).to_string(), json!(t.example_ids).to_string(),
                            json!(t.call_ids).to_string(), t.first_seen_unix, t.last_seen_unix, t.status
                        ],
                    )?;
                }
                Ok(ClusterStats { rows_clustered: pending.len(), themes_touched: touched.len() })
            })
        })
        .await
    }

    /// R4: themes seen at or after `since_unix`, optionally one kind and/or
    /// status, `count` desc then `last_seen` desc.
    pub async fn list_feedback_themes(
        &self,
        since_unix: i64,
        kind: Option<String>,
        status: Option<String>,
        limit: i64,
    ) -> Result<Vec<FeedbackTheme>, AppError> {
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {THEME_COLS} FROM feedback_themes WHERE last_seen_unix >= ?1 \
                 AND (?2 IS NULL OR kind = ?2) AND (?3 IS NULL OR status = ?3) \
                 ORDER BY count DESC, last_seen_unix DESC, theme_id LIMIT ?4"
            ))?;
            let rows = stmt.query_map(params![since_unix, kind, status, limit], FeedbackTheme::from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    /// One theme by id.
    pub async fn get_feedback_theme(&self, theme_id: String) -> Result<Option<FeedbackTheme>, AppError> {
        self.with_conn(move |conn| {
            Ok(conn
                .query_row(
                    &format!("SELECT {THEME_COLS} FROM feedback_themes WHERE theme_id = ?1"),
                    params![theme_id],
                    FeedbackTheme::from_row,
                )
                .optional()?)
        })
        .await
    }

    /// The `theme_id` a feedback row was clustered into (None before the
    /// nightly pass reaches it).
    pub async fn feedback_row_theme_id(&self, feedback_id: String) -> Result<Option<String>, AppError> {
        self.with_conn(move |conn| {
            Ok(conn
                .query_row("SELECT theme_id FROM feedback WHERE feedback_id = ?1", params![feedback_id], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .optional()?
                .flatten())
        })
        .await
    }

    /// R5: `(rows filed in the last 24 h, open themes)` -- counts only.
    pub async fn feedback_counts(&self, now_unix: i64) -> Result<(i64, i64), AppError> {
        self.with_conn(move |conn| {
            let rows: i64 = conn.query_row(
                "SELECT COUNT(*) FROM feedback WHERE created_unix >= ?1",
                params![now_unix - 86_400],
                |r| r.get(0),
            )?;
            let open: i64 = conn.query_row(
                "SELECT COUNT(*) FROM feedback_themes WHERE status = ?1",
                params![THEME_STATUS_OPEN],
                |r| r.get(0),
            )?;
            Ok((rows, open))
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Nightly task (R3).
// ---------------------------------------------------------------------------

/// The nightly pass runs at this UTC time, ahead of the midnight-PDT
/// (07:00 UTC) discovery chain that reads the themes.
const THEME_HOUR_UTC: i64 = 3;
const THEME_MINUTE_UTC: i64 = 10;

/// Seconds from `now_unix` to the next 03:10 UTC.
pub fn secs_until_next_theme_run(now_unix: i64) -> u64 {
    let today_run = now_unix.div_euclid(86_400) * 86_400 + THEME_HOUR_UTC * 3600 + THEME_MINUTE_UTC * 60;
    let next = if today_run > now_unix { today_run } else { today_run + 86_400 };
    (next - now_unix).max(0) as u64
}

/// Same "never joined, `tokio::spawn` loop" convention as
/// [`crate::retention::spawn_prune_scheduler`].
pub fn spawn_theme_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    spawn_theme_loop(state, None)
}

/// Test-only: the same unattended loop firing every `interval`.
pub fn spawn_theme_scheduler_for_test(
    state: AppState,
    interval: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    spawn_theme_loop(state, Some(interval))
}

fn spawn_theme_loop(state: AppState, interval: Option<std::time::Duration>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let sleep_for = interval
                .unwrap_or_else(|| std::time::Duration::from_secs(secs_until_next_theme_run(now_unix())));
            tokio::time::sleep(sleep_for).await;
            match state.db.cluster_feedback().await {
                Ok(s) if s.rows_clustered > 0 => {
                    tracing::info!(rows = s.rows_clustered, themes = s.themes_touched, "feedback themes clustered");
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "nightly feedback clustering failed"),
            }
        }
    })
}

crate::err_code! {
    pub FEEDBACK_NOT_FOUND,
    code: "feedback_not_found", family: Storage, class: AgentFault, retryable: false,
    remedy: "Use a feedback id this caller filed.",
}

