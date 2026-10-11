//! PRD-mcphost-paged-trait-on-every-list-verb: one paging contract for every
//! list verb -- the [`Paged`] trait, the one signed opaque keyset
//! [`Cursor`], the shared `limit` resolution, and [`PAGED_VERBS`], the set
//! the contract's `paged: true` and the drift test are both generated from.
//!
//! **Why a declarative macro, not a proc-macro.** The workspace has no
//! proc-macro crate (the PRD's open question defaults to a declarative macro
//! in that case). [`paged_verbs!`] is invoked exactly once, below: every
//! `Paged` impl, [`PAGED_VERBS`] and [`paged_spec`] come out of that single
//! list, so the set a handler is paged under and the set the contract
//! advertises cannot separate.
//!
//! **The cursor.** `base64url("v1:<seq>:<id>:<hmac8>")`, the tag being the
//! first 8 bytes of an HMAC-SHA256 under the per-process secret
//! `session_bind.rs` already holds, with the tenant id in the HMAC input. A
//! cursor minted before a restart, by another tenant, or typed by hand is
//! refused as `cursor_invalid` with `data.reason` one of `encoding`,
//! `version`, `signature`.

use base64::Engine as _;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::sync::Mutex;

use crate::errors::AppError;
use crate::session_bind::SessionBindings;

/// The remedy every `cursor_invalid` carries in `data.remedy`.
pub const CURSOR_REMEDY: &str = "omit cursor to restart from the first page";

/// Reserved key a paged handler leaves on its result when it clamped
/// `limit`; `handler::call_tool` removes it and reports it as
/// `_meta.mcphost.limit_clamped {asked, max}`.
pub const LIMIT_CLAMPED_KEY: &str = "__limit_clamped";

/// One description per paging property, defined once and spliced into every
/// paged verb's schema.
pub const LIMIT_DESCRIPTION: &str =
    "Page size, 1..=max for this verb (a larger value is clamped and reported in _meta.mcphost.limit_clamped).";
pub const CURSOR_DESCRIPTION: &str =
    "Opaque keyset cursor: pass the previous page's next_cursor verbatim; omit for the first page.";
pub const NEXT_CURSOR_DESCRIPTION: &str =
    "Opaque cursor for the next page; absent (key omitted) on the last page.";
pub const TRUNCATED_DESCRIPTION: &str = "true when more rows exist past this page.";

/// The keyset position of one row: `(seq, id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub seq: i64,
    pub id: String,
}

impl Cursor {
    pub fn new(seq: i64, id: impl Into<String>) -> Self {
        Cursor { seq, id: id.into() }
    }

    fn tag(secret: &SessionBindings, tenant_id: i64, body: &str) -> String {
        let mac = secret.mac(format!("{tenant_id}|{body}").as_bytes());
        mac[..8].iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Mint the opaque string for `tenant_id`.
    pub fn encode(&self, secret: &SessionBindings, tenant_id: i64) -> String {
        let body = format!("v1:{}:{}", self.seq, self.id);
        let tag = Self::tag(secret, tenant_id, &body);
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{body}:{tag}"))
    }

    /// Verify and open a cursor minted for `tenant_id` by this process.
    pub fn decode(raw: &str, secret: &SessionBindings, tenant_id: i64) -> Result<Cursor, AppError> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(raw.trim_end_matches('='))
            .map_err(|_| cursor_invalid("encoding"))?;
        let text = String::from_utf8(bytes).map_err(|_| cursor_invalid("encoding"))?;
        let (version, rest) = text.split_once(':').ok_or_else(|| cursor_invalid("encoding"))?;
        let digits = version.strip_prefix('v').unwrap_or("");
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(cursor_invalid("encoding"));
        }
        if version != "v1" {
            return Err(cursor_invalid("version"));
        }
        let (body_rest, tag) = rest.rsplit_once(':').ok_or_else(|| cursor_invalid("encoding"))?;
        let (seq, id) = body_rest.split_once(':').ok_or_else(|| cursor_invalid("encoding"))?;
        let seq: i64 = seq.parse().map_err(|_| cursor_invalid("encoding"))?;
        let body = format!("v1:{seq}:{id}");
        if Self::tag(secret, tenant_id, &body) != tag {
            return Err(cursor_invalid("signature"));
        }
        Ok(Cursor { seq, id: id.to_string() })
    }
}

/// `cursor_invalid {reason}` with the remedy in `data`.
pub fn cursor_invalid(reason: &'static str) -> AppError {
    AppError::Structured {
        code: "cursor_invalid",
        message: format!("cursor is not valid ({reason})"),
        data: json!({"reason": reason, "remedy": CURSOR_REMEDY}),
    }
}

static LEGACY_LOGGED: Mutex<Option<HashSet<i64>>> = Mutex::new(None);

/// Migration window: a pre-PRD cursor (`channel.read`'s bare integer,
/// `docs.list`'s bare name) is still accepted, logged once per tenant as
/// `cursor_legacy`. The caller supplies `legacy` -- how its verb used to read
/// the string -- and it is consulted only when the string is not a v1 cursor
/// at all (`encoding`); a well-formed cursor that fails its signature is
/// never second-guessed.
pub fn decode_or_legacy(
    raw: &str,
    secret: &SessionBindings,
    tenant_id: i64,
    verb: &str,
    legacy: impl FnOnce(&str) -> Option<Cursor>,
) -> Result<Cursor, AppError> {
    match Cursor::decode(raw, secret, tenant_id) {
        Ok(c) => Ok(c),
        Err(e) if e.code() == "cursor_invalid" && is_encoding(&e) => match legacy(raw) {
            Some(c) => {
                let mut guard = LEGACY_LOGGED.lock().unwrap_or_else(|p| p.into_inner());
                if guard.get_or_insert_with(HashSet::new).insert(tenant_id) {
                    tracing::info!(tenant_id, verb, "cursor_legacy");
                }
                Ok(c)
            }
            None => Err(e),
        },
        Err(e) => Err(e),
    }
}

fn is_encoding(e: &AppError) -> bool {
    matches!(e, AppError::Structured { data, .. } if data["reason"] == "encoding")
}

/// What a verb's `limit` resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedLimit {
    pub limit: i64,
    /// `Some((asked, max))` when the caller asked for more than `max`.
    pub clamped: Option<(i64, i64)>,
}

/// Requirement 7: absent -> `default`; above `max` -> clamped and reported;
/// below 1 (or not an integer) -> `args_invalid`.
pub fn resolve_limit(args: &Value, default: i64, max: i64) -> Result<ResolvedLimit, AppError> {
    let Some(raw) = args.get("limit").filter(|v| !v.is_null()) else {
        return Ok(ResolvedLimit { limit: default.min(max), clamped: None });
    };
    let asked = raw
        .as_i64()
        .ok_or_else(|| AppError::InvalidArgs("limit: must be an integer".to_string()))?;
    if asked < 1 {
        return Err(AppError::InvalidArgs(format!("limit: must be at least 1, got {asked}")));
    }
    if asked > max {
        return Ok(ResolvedLimit { limit: max, clamped: Some((asked, max)) });
    }
    Ok(ResolvedLimit { limit: asked, clamped: None })
}

/// Leave the clamp report on `value` for `handler::call_tool` to move into
/// `_meta.mcphost.limit_clamped`.
pub fn note_clamp(value: &mut Value, limit: &ResolvedLimit) {
    if let (Some((asked, max)), Some(obj)) = (limit.clamped, value.as_object_mut()) {
        obj.insert(LIMIT_CLAMPED_KEY.to_string(), json!({"asked": asked, "max": max}));
    }
}

/// Remove and return the clamp report [`note_clamp`] left.
pub fn take_clamp(value: &mut Value) -> Option<Value> {
    value.as_object_mut()?.remove(LIMIT_CLAMPED_KEY)
}

/// Requirement 3: `rows` was fetched with `limit + 1`. Trims it to `limit`
/// and returns the next cursor iff the extra row existed -- present exactly
/// on a non-last page.
pub fn split_page<T>(rows: &mut Vec<T>, limit: i64, cursor_of: impl Fn(&T) -> Cursor) -> Option<Cursor> {
    if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last().map(cursor_of)
    } else {
        None
    }
}

/// Write `next_cursor`/`truncated` onto a page: the key is omitted (not
/// null) on the last page.
pub fn finish_page(
    out: &mut Map<String, Value>,
    next: Option<&Cursor>,
    secret: &SessionBindings,
    tenant_id: i64,
) {
    if let Some(c) = next {
        out.insert("next_cursor".to_string(), json!(c.encode(secret, tenant_id)));
    }
}

/// A verb that pages. Implemented only through [`paged_verbs!`].
pub trait Paged {
    const VERB: &'static str;
    const KEY: &'static str;
    const DEFAULT_LIMIT: u32;
    const MAX_LIMIT: u32;
    type Row;
    fn cursor_of(row: &Self::Row) -> Cursor;

    /// Input properties this verb's schema gets.
    fn input_properties() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(
            "limit".into(),
            json!({
                "type": "integer", "minimum": 1, "maximum": Self::MAX_LIMIT,
                "default": Self::DEFAULT_LIMIT, "description": LIMIT_DESCRIPTION,
            }),
        );
        m.insert("cursor".into(), json!({"type": "string", "description": CURSOR_DESCRIPTION}));
        m
    }

    /// Output properties this verb returns (reserved for the typed output
    /// schema; also what the contract documents).
    fn output_properties() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(Self::KEY.into(), json!({"type": "array"}));
        m.insert("next_cursor".into(), json!({"type": "string", "description": NEXT_CURSOR_DESCRIPTION}));
        m.insert("truncated".into(), json!({"type": "boolean", "description": TRUNCATED_DESCRIPTION}));
        m
    }
}

/// Everything the contract needs to know about a paged verb.
#[derive(Debug, Clone, Copy)]
pub struct PagedSpec {
    pub verb: &'static str,
    pub key: &'static str,
    pub default_limit: u32,
    pub max_limit: u32,
    input_properties: fn() -> Map<String, Value>,
}

macro_rules! paged_verbs {
    ($( $name:ident { verb: $verb:literal, key: $key:literal, default: $def:literal, max: $max:literal,
                      row: $row:ty, cursor_of: $cur:expr } )+) => {
        $(
            pub struct $name;
            impl Paged for $name {
                const VERB: &'static str = $verb;
                const KEY: &'static str = $key;
                const DEFAULT_LIMIT: u32 = $def;
                const MAX_LIMIT: u32 = $max;
                type Row = $row;
                fn cursor_of(row: &Self::Row) -> Cursor { ($cur)(row) }
            }
        )+
        /// Every canonical verb that derives [`Paged`], generated from the
        /// one [`paged_verbs!`] list.
        pub const PAGED_VERBS: &[&str] = &[$( $verb ),+];
        const PAGED_SPECS: &[PagedSpec] = &[$(
            PagedSpec { verb: $verb, key: $key, default_limit: $def, max_limit: $max,
                        input_properties: <$name as Paged>::input_properties }
        ),+];
    };
}

paged_verbs! {
    ChannelRead { verb: "host.channel.read", key: "posts", default: 50, max: 100,
        row: crate::db::ChannelPostRow, cursor_of: |p: &crate::db::ChannelPostRow| Cursor::new(p.seq, "") }
    MsgInbox { verb: "host.msg.inbox", key: "messages", default: 50, max: 100,
        row: crate::db::MessageRow, cursor_of: |m: &crate::db::MessageRow| Cursor::new(m.created_unix_ms, m.id.clone()) }
    MsgThread { verb: "host.msg.thread", key: "messages", default: 50, max: 100,
        row: crate::db::MessageRow, cursor_of: |m: &crate::db::MessageRow| Cursor::new(m.seq, "") }
    DocsList { verb: "host.docs.list", key: "documents", default: 100, max: 1000,
        row: crate::db::DocumentRow, cursor_of: |d: &crate::db::DocumentRow| Cursor::new(d.seq, d.name.clone()) }
}

/// The [`PagedSpec`] for a canonical verb, `None` if it does not page.
pub fn paged_spec(verb: &str) -> Option<&'static PagedSpec> {
    PAGED_SPECS.iter().find(|s| s.verb == verb)
}

/// Verbs that return a top-level array and deliberately do not (yet) derive
/// [`Paged`], each with the `unpaged_reason` the contract entry carries
/// (>= 20 characters). The drift test fails any array-returning verb that is
/// in neither this table nor [`PAGED_VERBS`].
const UNPAGED_LEGACY: &str =
    "keeps its own pre-Paged cursor encoding; converges onto paged::Cursor in a follow-up migration";
const UNPAGED_WINDOW: &str =
    "limit-only window over a bounded recent set; no keyset cursor is offered yet";
const UNPAGED_EXPORT: &str =
    "export or arbitrary-query semantics: the caller bounds the result, so no page cursor applies";
const UNPAGED_BOUNDED: &str =
    "tenant-quota-bounded set returned whole in one response, never more than a plan-capped few hundred rows";
const UNPAGED_RANKED: &str =
    "relevance-ranked top-k result, not a keyset-ordered list, so there is no stable cursor";
const UNPAGED_ECHO: &str =
    "not a list verb: the array is a field of a single-record or mutation response";
const UNPAGED_WAIT: &str =
    "long-poll batch of at most 50 rows that advances the shared cursor; wait semantics, not page-through";

pub const UNPAGED_VERBS: &[(&str, &str)] = &[
    ("billing.plans", UNPAGED_ECHO),
    ("host.agent.contacts", UNPAGED_BOUNDED),
    ("host.agent.profile_set", UNPAGED_ECHO),
    ("host.agent.search", UNPAGED_LEGACY),
    ("host.audit.chain", UNPAGED_WINDOW),
    ("host.catalog.search", UNPAGED_WINDOW),
    ("host.changelog", UNPAGED_EXPORT),
    ("host.docs.search", UNPAGED_RANKED),
    ("host.drift.reviews", UNPAGED_WINDOW),
    ("host.enduser.audit", UNPAGED_LEGACY),
    ("host.enduser.list", UNPAGED_LEGACY),
    ("host.group.list", UNPAGED_BOUNDED),
    ("host.invite.create", UNPAGED_ECHO),
    ("host.invite.list", UNPAGED_BOUNDED),
    ("host.msg.wait", UNPAGED_WAIT),
    ("host.oauth.audit", UNPAGED_LEGACY),
    ("host.oauth.audit_export", UNPAGED_EXPORT),
    ("host.oauth.grants", UNPAGED_BOUNDED),
    ("host.oauth.issuers", UNPAGED_BOUNDED),
    ("host.oauth.pending", UNPAGED_BOUNDED),
    ("host.oauth.policy", UNPAGED_BOUNDED),
    ("host.oauth.policy_set", UNPAGED_ECHO),
    ("host.oauth.scopes", UNPAGED_BOUNDED),
    ("host.oauth.trusted_issuers", UNPAGED_BOUNDED),
    ("host.policy.list", UNPAGED_BOUNDED),
    ("host.runs.list", UNPAGED_WINDOW),
    ("host.secret.list", UNPAGED_BOUNDED),
    ("host.state.list", UNPAGED_WINDOW),
    ("host.state.query", UNPAGED_WINDOW),
    ("host.table.charts", UNPAGED_BOUNDED),
    ("host.table.graph", UNPAGED_BOUNDED),
    ("host.table.handles", UNPAGED_BOUNDED),
    ("host.table.list", UNPAGED_BOUNDED),
    ("host.table.models", UNPAGED_BOUNDED),
    ("host.table.next_questions", UNPAGED_EXPORT),
    ("host.table.query", UNPAGED_EXPORT),
    ("host.table.query_log", UNPAGED_WINDOW),
    ("host.table.query_stats", UNPAGED_BOUNDED),
    ("host.tool.list", UNPAGED_BOUNDED),
    ("host.trigger.list", UNPAGED_BOUNDED),
    ("host.vault.providers", UNPAGED_BOUNDED),
    ("host.whoami", UNPAGED_ECHO),
];


/// The `unpaged_reason` for `verb`, if excused.
pub fn unpaged_reason(verb: &str) -> Option<&'static str> {
    UNPAGED_VERBS.iter().find(|(v, _)| *v == verb).map(|(_, r)| *r)
}

/// Splice the generated paging properties into a verb's input schema
/// (`properties` object), replacing any hand-typed `limit`/`cursor`.
pub fn splice_input_schema(verb: &str, properties: &mut Map<String, Value>) {
    let Some(spec) = paged_spec(verb) else { return };
    let props = (spec.input_properties)();
    for (k, v) in props {
        properties.insert(k, v);
    }
}

/// The drift check: every contract entry whose recorded response has a
/// top-level array must be `paged: true` or carry `unpaged_reason` of at
/// least 20 characters. `contract_tools` is the contract's `tools` array;
/// `recorded` maps a verb to its recorded `structuredContent`. Returns one
/// message per offending verb, each naming it.
pub fn drift_violations(contract_tools: &[Value], recorded: &[(String, Value)]) -> Vec<String> {
    let mut out = Vec::new();
    for (verb, response) in recorded {
        if !has_top_level_array(response) {
            continue;
        }
        let entry = contract_tools.iter().find(|t| t["name"] == json!(verb));
        let ok = entry.is_some_and(|e| {
            e["paged"] == json!(true)
                || e["unpaged_reason"].as_str().is_some_and(|r| r.chars().count() >= 20)
        });
        if !ok {
            out.push(format!(
                "{verb}: returns a top-level array but is neither `paged: true` nor carries an \
                 `unpaged_reason` (>= 20 chars) -- derive Paged in src/paged.rs or excuse it"
            ));
        }
    }
    out
}

/// Whether `response` is an object with at least one top-level array value.
pub fn has_top_level_array(response: &Value) -> bool {
    response.as_object().is_some_and(|o| o.values().any(Value::is_array))
}
