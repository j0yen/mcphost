//! PRD-mcphost-notes-to-future-self: `host.note.put` / `host.note.list` /
//! `host.note.delete` over the `notes/` prefix of `tenant_state_kv` (the
//! store first-contact notes already use, [`crate::tenant_state`]), and the
//! ` — note to self: ` suffix a tool-attached note adds to that tool's
//! `tools/list` description. No new table: `state_bytes_max` and
//! `host.state.*` keep working over the same rows.
//!
//! A `notes/` value is read one way only, [`Note::from_value`]: the JSON
//! shape this module writes, or the legacy first-contact shape
//! (`{text, source, at}`) / a bare string, both read as an unattached note.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;

pub const NOTE_PREFIX: &str = "notes/";
/// R1: the largest `text`, in bytes.
pub const TEXT_MAX_BYTES: usize = 4096;
/// R1: `ttl_s` bounds (a minute to ninety days).
pub const TTL_MIN_S: i64 = 60;
pub const TTL_MAX_S: i64 = 7_776_000;
/// R3: the description suffix separator and the suffix text's length cap.
pub const SUFFIX_SEPARATOR: &str = " — note to self: ";
pub const SUFFIX_MAX_CHARS: usize = 120;
const LIST_LIMIT_DEFAULT: usize = 100;
const LIST_LIMIT_MAX: usize = 100;
/// More rows than any plan's `notes_max` lets a tenant hold.
const SCAN_LIMIT: i64 = 100_000;

/// What a note is attached to.
#[derive(Debug, Clone, PartialEq)]
pub enum Attach {
    Tool(String),
    Run(String),
}

impl Attach {
    fn to_json(&self) -> Value {
        match self {
            Attach::Tool(t) => json!({"tool": t}),
            Attach::Run(r) => json!({"run": r}),
        }
    }

    fn from_json(v: &Value) -> Option<Attach> {
        let obj = v.as_object()?;
        if let Some(t) = obj.get("tool").and_then(Value::as_str) {
            return Some(Attach::Tool(t.to_string()));
        }
        obj.get("run").and_then(Value::as_str).map(|r| Attach::Run(r.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub key: String,
    pub text: String,
    pub attach_to: Option<Attach>,
    pub created_unix: i64,
    pub expires_unix: Option<i64>,
}

impl Note {
    /// The legacy-text constructor: a first-contact `{text, source, at}`
    /// object or a bare string, unattached and never expiring.
    pub fn from_legacy(key: &str, value: &Value, updated_unix: i64) -> Note {
        let text = match value {
            Value::String(s) => s.clone(),
            other => other.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
        };
        let created_unix = value.get("at").and_then(Value::as_i64).unwrap_or(updated_unix);
        Note { key: key.to_string(), text, attach_to: None, created_unix, expires_unix: None }
    }

    /// The only way a `notes/` row is read.
    pub fn from_value(key: &str, value: &Value, updated_unix: i64) -> Note {
        if value.get("created_unix").and_then(Value::as_i64).is_none() {
            return Note::from_legacy(key, value, updated_unix);
        }
        Note {
            key: key.to_string(),
            text: value.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
            attach_to: value.get("attach_to").and_then(Attach::from_json),
            created_unix: value.get("created_unix").and_then(Value::as_i64).unwrap_or(updated_unix),
            expires_unix: value.get("expires_unix").and_then(Value::as_i64),
        }
    }

    pub fn is_live(&self, now: i64) -> bool {
        self.expires_unix.is_none_or(|e| e > now)
    }

    fn to_list_json(&self) -> Value {
        json!({
            "key": self.key,
            "text": self.text,
            "attach_to": self.attach_to.as_ref().map(Attach::to_json),
            "created_unix": self.created_unix,
            "expires_unix": self.expires_unix,
        })
    }
}

/// The clock every note read and write uses: the host clock plus the test
/// offset `blobs::now` already carries (0 in production).
pub fn now(state: &AppState) -> i64 {
    crate::blobs::now(state)
}

/// Every live note of `tenant_id`, newest first.
pub async fn live_notes(state: &AppState, tenant_id: i64, now: i64) -> Result<Vec<Note>, AppError> {
    let rows = state
        .db
        .state_kv_list(tenant_id, Some(NOTE_PREFIX.to_string()), SCAN_LIMIT, String::new())
        .await?;
    let mut notes: Vec<Note> = rows
        .into_iter()
        .map(|(key, value_json, updated_unix)| {
            let value: Value = serde_json::from_str(&value_json).unwrap_or(Value::Null);
            Note::from_value(&key, &value, updated_unix)
        })
        .filter(|n| n.is_live(now))
        .collect();
    notes.sort_by(|a, b| b.created_unix.cmp(&a.created_unix).then_with(|| b.key.cmp(&a.key)));
    Ok(notes)
}

/// R3: `text` cut to [`SUFFIX_MAX_CHARS`] chars at a char boundary, the
/// last of them `…` when it was cut.
pub fn suffix_text(text: &str) -> String {
    if text.chars().count() <= SUFFIX_MAX_CHARS {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(SUFFIX_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

pub fn describe_with_note(description: &str, note_text: &str) -> String {
    format!("{description}{SUFFIX_SEPARATOR}{}", suffix_text(note_text))
}

/// Local tool name -> the text of its newest live attached note.
pub async fn tool_note_texts(
    state: &AppState,
    tenant_id: i64,
    now: i64,
) -> Result<HashMap<String, String>, AppError> {
    let mut out = HashMap::new();
    for note in live_notes(state, tenant_id, now).await? {
        if let Some(Attach::Tool(tool)) = &note.attach_to {
            out.entry(tool.clone()).or_insert(note.text);
        }
    }
    Ok(out)
}

fn invalid(msg: &str) -> AppError {
    AppError::InvalidArgs(msg.to_string())
}

fn parse_attach(args: &Value) -> Result<Option<Attach>, AppError> {
    let Some(raw) = args.get("attach_to").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let obj = raw.as_object().ok_or_else(|| invalid("attach_to must be {tool: name} or {run: id}"))?;
    let tool = obj.get("tool").and_then(Value::as_str);
    let run = obj.get("run").and_then(Value::as_str);
    match (tool, run) {
        (Some(t), None) if !t.is_empty() => Ok(Some(Attach::Tool(t.to_string()))),
        (None, Some(r)) if !r.is_empty() => Ok(Some(Attach::Run(r.to_string()))),
        _ => Err(invalid("attach_to must name exactly one of tool or run")),
    }
}

fn not_found(what: &str, name: &str) -> AppError {
    AppError::Structured {
        code: crate::subkeys::NOT_FOUND,
        message: format!("no {what} '{name}' for this tenant"),
        data: json!({what: name}),
    }
}

fn quota_exceeded(limit: i64, used: i64) -> AppError {
    AppError::Structured {
        code: crate::errors::QUOTA_EXCEEDED,
        message: format!("notes quota exceeded: notes_max (limit {limit}, used {used})"),
        data: json!({"quota": "notes_max", "limit": limit, "used": used}),
    }
}

/// R1.
pub async fn put(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| invalid("missing required argument 'text'"))?;
    if text.len() > TEXT_MAX_BYTES {
        return Err(invalid(&format!("text is {} bytes; the limit is {TEXT_MAX_BYTES}", text.len())));
    }
    let attach_to = parse_attach(args)?;
    let ttl_s = match args.get("ttl_s").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => {
            let ttl = v.as_i64().ok_or_else(|| invalid("ttl_s must be an integer"))?;
            if !(TTL_MIN_S..=TTL_MAX_S).contains(&ttl) {
                return Err(invalid(&format!("ttl_s must be between {TTL_MIN_S} and {TTL_MAX_S}")));
            }
            Some(ttl)
        }
    };

    let missing = match &attach_to {
        Some(Attach::Tool(name)) => state.db.get_tool(tenant.id, name.clone()).await?.is_none().then_some(("tool", name)),
        Some(Attach::Run(id)) => state.db.get_run(id.clone(), tenant.id).await?.is_none().then_some(("run", id)),
        None => None,
    };
    if let Some((what, name)) = missing {
        return Err(not_found(what, name));
    }

    let now = now(state);
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
    })?;
    let used = live_notes(state, tenant.id, now).await?.len() as i64;
    if used >= plan.notes_max {
        return Err(quota_exceeded(plan.notes_max, used));
    }

    let key = format!("{NOTE_PREFIX}{}", crate::state::new_ulid());
    let expires_unix = ttl_s.map(|t| now + t);
    let value = json!({
        "text": text,
        "attach_to": attach_to.as_ref().map(Attach::to_json),
        "created_unix": now,
        "expires_unix": expires_unix,
    });
    crate::tenant_state::state_set(state, tenant, &json!({"key": key.clone(), "value": value}), None).await?;
    let visible_in = match attach_to {
        Some(Attach::Tool(_)) => "tools/list",
        _ => "host.note.list",
    };
    Ok(json!({
        "key": key,
        "attach_to": attach_to.as_ref().map(Attach::to_json),
        "expires_at": expires_unix.map(crate::state::rfc3339_from_unix),
        "visible_in": visible_in,
    }))
}

/// R2: newest first; `cursor` is the key of the last note of the previous
/// page.
pub async fn list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .map_or(LIST_LIMIT_DEFAULT, |l| l.clamp(1, LIST_LIMIT_MAX as i64) as usize);
    let filter = parse_attach(args)?;
    let cursor = args.get("cursor").and_then(Value::as_str);
    let mut notes = live_notes(state, tenant.id, now(state)).await?;
    if let Some(f) = &filter {
        notes.retain(|n| n.attach_to.as_ref() == Some(f));
    }
    if let Some(c) = cursor {
        let after = notes.iter().position(|n| n.key == c).map_or(0, |i| i + 1);
        notes.drain(..after);
    }
    let more = notes.len() > limit;
    notes.truncate(limit);
    let next_cursor = if more { notes.last().map(|n| n.key.clone()) } else { None };
    Ok(json!({
        "notes": notes.iter().map(Note::to_list_json).collect::<Vec<_>>(),
        "next_cursor": next_cursor,
    }))
}

/// R2.
pub async fn delete(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let key = args
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("missing required argument 'key'"))?;
    if !key.starts_with(NOTE_PREFIX) {
        return Err(invalid("key must be a notes/ key from host.note.list"));
    }
    let attach_to = state
        .db
        .state_kv_get(tenant.id, key.to_string(), String::new())
        .await?
        .and_then(|(value_json, updated)| {
            let value: Value = serde_json::from_str(&value_json).ok()?;
            Note::from_value(key, &value, updated).attach_to
        });
    let deleted = state.db.state_kv_delete(tenant.id, key.to_string(), String::new()).await?;
    Ok(json!({
        "key": key,
        "deleted": deleted,
        "attach_to": attach_to.as_ref().map(Attach::to_json),
    }))
}

/// R3: whether a `put`/`delete` result changed what a tool's `tools/list`
/// description says, i.e. the session should hear `list_changed`.
pub fn changes_tool_list(tool_name: &str, result: &Value) -> bool {
    matches!(tool_name, "host.note.put" | "host.note.delete")
        && result.get("deleted").and_then(Value::as_bool) != Some(false)
        && result.pointer("/attach_to/tool").is_some_and(Value::is_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_cuts_at_a_char_boundary_with_an_ellipsis() {
        let long: String = "é".repeat(300);
        let cut = suffix_text(&long);
        assert_eq!(cut.chars().count(), SUFFIX_MAX_CHARS);
        assert!(cut.ends_with('…'));
        assert_eq!(suffix_text("short"), "short");
    }

    #[test]
    fn legacy_first_contact_value_reads_as_an_unattached_note() {
        let v = json!({"text": "hi", "source": "first_call", "at": 5});
        let n = Note::from_value("notes/x", &v, 9);
        assert_eq!((n.text.as_str(), n.created_unix, n.attach_to), ("hi", 5, None));
    }
}
