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
//! **The cursor.** `base64url("v1:<order>:<seq>:<id>:<hmac8>")`, the tag being the
//! first 8 bytes of an HMAC-SHA256 under the per-process secret
//! `session_bind.rs` already holds, with the tenant id in the HMAC input. A
//! cursor minted before a restart, by another tenant, or typed by hand is
//! refused as `cursor_invalid` with `data.reason` one of `encoding`,
//! `version`, `signature`; one minted under a different `order` than the
//! call passes is refused with reason `order`.
//!
//! **Declared orderings (PRD-mcphost-declared-ordering-per-verb).** Each
//! `Paged` verb names an [`OrderEnum`] ([`order_enum!`]); the schema's
//! `order` property, the SQL `ORDER BY` ([`Paged::sql_order`]) and the keyset
//! predicate ([`keyset_clause`]) are all generated from that one declaration.

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

/// Reserved key a handler leaves on its result when the caller used a
/// deprecated argument; `handler::call_tool` removes it and reports it as
/// `_meta.deprecated`.
pub const DEPRECATED_ARG_KEY: &str = "__deprecated_arg";

/// One description per paging property, defined once and spliced into every
/// paged verb's schema.
pub const LIMIT_DESCRIPTION: &str =
    "Page size, 1..=max for this verb (a larger value is clamped and reported in _meta.mcphost.limit_clamped).";
pub const CURSOR_DESCRIPTION: &str =
    "Opaque keyset cursor: pass the previous page's next_cursor verbatim; omit for the first page.";
pub const NEXT_CURSOR_DESCRIPTION: &str =
    "Opaque cursor for the next page; absent (key omitted) on the last page.";
pub const TRUNCATED_DESCRIPTION: &str = "true when more rows exist past this page.";
pub const FIELDS_DESCRIPTION: &str = "Return only these row fields; default all.";

/// PRD-mcphost-count-and-total-semantics R1: a filtered set of at most this
/// many rows is counted exactly; a larger one gets a labelled estimate.
pub const EXACT_TOTAL_MAX: u64 = 10_000;

/// R2: the two sentences every page's `total`/`total_estimate` schema
/// properties (and `llms.txt`) carry, generated from [`EXACT_TOTAL_MAX`].
pub fn total_description() -> String {
    format!("Exact row count of the filtered set; present only when the set has at most {EXACT_TOTAL_MAX} rows.")
}
pub fn total_estimate_description() -> String {
    format!(
        "Estimated row count (exact: false); present instead of total when the set has more than {EXACT_TOTAL_MAX} rows."
    )
}

/// The one counter behind a page's `total`, `host.table.count` and
/// `host.table.list.rows`: `n` is exact iff `exact`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Count {
    pub exact: bool,
    pub n: u64,
}

impl Count {
    /// `bounded` is `SELECT count(*) FROM (<filtered> LIMIT EXACT_TOTAL_MAX+1)`.
    /// At most [`EXACT_TOTAL_MAX`] is exact; above it `estimate` supplies `n`.
    pub fn from_bounded(bounded: u64, estimate: impl FnOnce() -> u64) -> Count {
        if bounded <= EXACT_TOTAL_MAX {
            Count { exact: true, n: bounded }
        } else {
            Count { exact: false, n: estimate().max(bounded) }
        }
    }

    /// `total: n` when exact, else `total_estimate: n, exact: false`; never both.
    pub fn insert_into(&self, out: &mut Map<String, Value>) {
        if self.exact {
            out.insert("total".to_string(), json!(self.n));
        } else {
            out.insert("total_estimate".to_string(), json!(self.n));
            out.insert("exact".to_string(), json!(false));
        }
    }
}

/// Row estimate for `table`: `sqlite_stat1` when `ANALYZE` has run, else
/// `max(rowid)`.
pub fn estimate_rows_sync(conn: &rusqlite::Connection, table: &str) -> u64 {
    let from_stat: Option<i64> = conn
        .query_row(
            "SELECT CAST(substr(stat, 1, instr(stat || ' ', ' ') - 1) AS INTEGER) FROM sqlite_stat1 \
             WHERE tbl = ?1 AND stat IS NOT NULL ORDER BY 1 DESC LIMIT 1",
            [table],
            |r| r.get(0),
        )
        .ok();
    if let Some(n) = from_stat.filter(|n| *n > 0) {
        return n as u64;
    }
    let quoted = table.replace('"', "\"\"");
    conn.query_row(&format!("SELECT COALESCE(max(rowid), 0) FROM \"{quoted}\""), [], |r| r.get::<_, i64>(0))
        .map(|n| n.max(0) as u64)
        .unwrap_or(0)
}

/// The keyset position of one row: `(seq, id)`, minted under one `order`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub seq: i64,
    pub id: String,
    /// The wire name of the [`OrderEnum`] variant the cursor was minted
    /// under (empty until [`Cursor::with_order`] stamps it).
    pub order: String,
}

impl Cursor {
    pub fn new(seq: i64, id: impl Into<String>) -> Self {
        Cursor { seq, id: id.into(), order: String::new() }
    }

    /// Stamp the order this cursor is minted under.
    pub fn with_order(mut self, order: impl Into<String>) -> Self {
        self.order = order.into();
        self
    }

    /// Requirement 3: a cursor minted under one order passed with another is
    /// `cursor_invalid {reason: "order"}`.
    pub fn require_order(&self, want: &str) -> Result<(), AppError> {
        if self.order == want { Ok(()) } else { Err(cursor_invalid("order")) }
    }

    fn tag(secret: &SessionBindings, tenant_id: i64, body: &str) -> String {
        let mac = secret.mac(format!("{tenant_id}|{body}").as_bytes());
        mac[..8].iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Mint the opaque string for `tenant_id`.
    pub fn encode(&self, secret: &SessionBindings, tenant_id: i64) -> String {
        let body = format!("v1:{}:{}:{}", self.order, self.seq, self.id);
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
        let (order, rest) = body_rest.split_once(':').ok_or_else(|| cursor_invalid("encoding"))?;
        let (seq, id) = rest.split_once(':').ok_or_else(|| cursor_invalid("encoding"))?;
        let seq: i64 = seq.parse().map_err(|_| cursor_invalid("encoding"))?;
        let body = format!("v1:{order}:{seq}:{id}");
        if Self::tag(secret, tenant_id, &body) != tag {
            return Err(cursor_invalid("signature"));
        }
        Ok(Cursor { seq, id: id.to_string(), order: order.to_string() })
    }
}

/// `cursor_invalid {reason}` with the remedy in `data`.
pub fn cursor_invalid(reason: &'static str) -> AppError {
    AppError::Structured {
        code: CURSOR_INVALID,
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

/// Leave a deprecated-argument report for `handler::call_tool` to move into
/// `_meta.deprecated {argument, replaced_by, sunset}`.
pub fn note_deprecated_arg(value: &mut Value, argument: &str, replaced_by: &str, sunset: &str) {
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            DEPRECATED_ARG_KEY.to_string(),
            json!({"argument": argument, "replaced_by": replaced_by, "sunset": sunset}),
        );
    }
}

/// Remove and return the report [`note_deprecated_arg`] left.
pub fn take_deprecated_arg(value: &mut Value) -> Option<Value> {
    value.as_object_mut()?.remove(DEPRECATED_ARG_KEY)
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

/// Sort direction of one declared order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Asc,
    Desc,
}

impl Dir {
    fn sql(self) -> &'static str {
        match self {
            Dir::Asc => "ASC",
            Dir::Desc => "DESC",
        }
    }

    fn cmp(self) -> &'static str {
        match self {
            Dir::Asc => ">",
            Dir::Desc => "<",
        }
    }
}

/// How the keyset predicate for an order is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// `(col, id_col) <cmp> (cursor.seq, cursor.id)`: `col` may repeat, the
    /// id column breaks the tie.
    Tuple,
    /// `col <cmp> cursor.seq`: `col` is an integer that is unique in the
    /// verb's row set, so it is its own tiebreak.
    UniqueInt,
    /// `col <cmp> cursor.id`: `col` is text that is unique in the verb's row
    /// set (the cursor's `id` carries it).
    UniqueText,
}

/// One declared ordering: its wire name, the sort column, the id tiebreak
/// column and the direction.
#[derive(Debug, Clone, Copy)]
pub struct OrderSpec {
    pub name: &'static str,
    pub col: &'static str,
    pub id_col: &'static str,
    pub dir: Dir,
    pub key: KeyKind,
}

/// The orderings a verb offers, generated by [`order_enum!`].
pub trait OrderEnum: Copy + PartialEq + std::fmt::Debug + 'static {
    const ALL: &'static [Self];
    const DEFAULT: Self;
    fn spec(self) -> OrderSpec;

    fn name(self) -> &'static str {
        self.spec().name
    }

    /// `(sort column, direction)`.
    fn order_by(self) -> (&'static str, Dir) {
        let s = self.spec();
        (s.col, s.dir)
    }

    fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|o| o.name() == raw)
    }

    fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|o| o.name()).collect()
    }

    /// `ORDER BY <col> <dir>, <id> <dir>`.
    fn sql_order(self) -> String {
        let s = self.spec();
        format!("ORDER BY {} {}, {} {}", s.col, s.dir.sql(), s.id_col, s.dir.sql())
    }

    /// The input-schema `order` property.
    fn schema_property() -> Value {
        json!({
            "type": "string",
            "enum": Self::names(),
            "default": Self::DEFAULT.name(),
            "description": format!("Row order; default {}.", Self::DEFAULT.name()),
        })
    }
}

/// Define a verb's ordering enum. The variant list is the only place a
/// column, direction or wire name is written.
macro_rules! order_enum {
    ($name:ident { default: $def:ident; $( $var:ident => ($lit:literal, $col:literal, $idc:literal, $dir:ident, $key:ident) ),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $( $var ),+ }
        impl OrderEnum for $name {
            const ALL: &'static [Self] = &[ $( Self::$var ),+ ];
            const DEFAULT: Self = Self::$def;
            fn spec(self) -> OrderSpec {
                match self {
                    $( Self::$var => OrderSpec {
                        name: $lit, col: $col, id_col: $idc, dir: Dir::$dir, key: KeyKind::$key,
                    } ),+
                }
            }
        }
    };
}

order_enum! {
    DocsOrder {
        default: CreatedDesc;
        CreatedDesc => ("created_desc", "seq", "id", Desc, Tuple),
        CreatedAsc => ("created_asc", "seq", "id", Asc, Tuple),
        NameAsc => ("name_asc", "name", "id", Asc, UniqueText),
    }
}
order_enum! { InboxOrder { default: CreatedAsc; CreatedAsc => ("created_asc", "m.created_unix_ms", "m.id", Asc, Tuple) } }
order_enum! { ThreadOrder { default: SeqAsc; SeqAsc => ("seq_asc", "m.seq", "m.id", Asc, UniqueInt) } }
order_enum! { ChannelOrder { default: SeqAsc; SeqAsc => ("seq_asc", "seq", "id", Asc, UniqueInt) } }
order_enum! {
    RunsOrder {
        default: CreatedDesc;
        CreatedDesc => ("created_desc", "rowid", "id", Desc, Tuple),
        CreatedAsc => ("created_asc", "rowid", "id", Asc, Tuple),
    }
}

order_enum! {
    CallsOrder {
        default: NewestFirst;
        NewestFirst => ("newest_first", "c.id", "c.id", Desc, UniqueInt),
    }
}

/// Resolve a call's `order`: absent/null -> the enum's default; an unknown
/// name -> `args_invalid` listing the declared ones.
pub fn resolve_order<O: OrderEnum>(args: &Value) -> Result<O, AppError> {
    let Some(raw) = args.get("order").filter(|v| !v.is_null()) else {
        return Ok(O::DEFAULT);
    };
    raw.as_str().and_then(O::parse).ok_or_else(|| {
        AppError::InvalidArgs(format!("order: must be one of {}", O::names().join(", ")))
    })
}

/// The keyset predicate for resuming after `cursor` under `order`, with its
/// bind values; placeholders are numbered from `first_idx`.
pub fn keyset_clause<O: OrderEnum>(
    order: O,
    cursor: &Cursor,
    first_idx: usize,
) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value as Sql;
    let s = order.spec();
    let cmp = s.dir.cmp();
    match s.key {
        KeyKind::Tuple => (
            format!("({}, {}) {cmp} (?{first_idx}, ?{})", s.col, s.id_col, first_idx + 1),
            vec![Sql::Integer(cursor.seq), Sql::Text(cursor.id.clone())],
        ),
        KeyKind::UniqueInt => (format!("{} {cmp} ?{first_idx}", s.col), vec![Sql::Integer(cursor.seq)]),
        KeyKind::UniqueText => (format!("{} {cmp} ?{first_idx}", s.col), vec![Sql::Text(cursor.id.clone())]),
    }
}

/// A verb that pages. Implemented only through [`paged_verbs!`].
pub trait Paged {
    const VERB: &'static str;
    const KEY: &'static str;
    const DEFAULT_LIMIT: u32;
    const MAX_LIMIT: u32;
    type Row;
    type Order: OrderEnum;
    /// PRD-mcphost-fields-projection-on-list-verbs R1: the row keys a caller
    /// may project to. Listed once, in [`paged_verbs!`]; `fldp_ac04` asserts
    /// it equals the key set the handler really serializes.
    const FIELDS: &'static [&'static str];
    /// The keyset position of `row` under `order` (unstamped).
    fn cursor_of(row: &Self::Row, order: Self::Order) -> Cursor;

    /// [`Paged::cursor_of`], stamped with `order` so a cursor cannot be
    /// replayed under another one.
    fn cursor_at(row: &Self::Row, order: Self::Order) -> Cursor {
        Self::cursor_of(row, order).with_order(order.name())
    }

    /// `(sort column, "id")`: the keyset columns for `order`.
    fn keyset(order: Self::Order) -> (&'static str, &'static str) {
        let s = order.spec();
        (s.col, s.id_col)
    }

    /// `ORDER BY <col> <dir>, id <dir>` -- the only place a list query's
    /// ordering is written.
    fn sql_order(order: Self::Order) -> String {
        order.sql_order()
    }

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
        m.insert("order".into(), <Self::Order as OrderEnum>::schema_property());
        m.insert("fields".into(), fields_property(Self::FIELDS));
        m
    }

    /// Output properties this verb returns (reserved for the typed output
    /// schema; also what the contract documents).
    fn output_properties() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(Self::KEY.into(), json!({"type": "array"}));
        m.insert("next_cursor".into(), json!({"type": "string", "description": NEXT_CURSOR_DESCRIPTION}));
        m.insert("truncated".into(), json!({"type": "boolean", "description": TRUNCATED_DESCRIPTION}));
        m.insert("total".into(), json!({"type": "integer", "description": total_description()}));
        m.insert("total_estimate".into(), json!({"type": "integer", "description": total_estimate_description()}));
        m.insert("exact".into(), json!({"type": "boolean", "description": "false when only total_estimate is given."}));
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
    /// Declared order names, in declaration order, and the default.
    pub orders: fn() -> Vec<&'static str>,
    pub default_order: fn() -> &'static str,
    pub fields: &'static [&'static str],
    input_properties: fn() -> Map<String, Value>,
}

macro_rules! paged_verbs {
    ($( $name:ident { verb: $verb:literal, key: $key:literal, default: $def:literal, max: $max:literal,
                      row: $row:ty, order: $ord:ty, fields: [$($field:literal),+ $(,)?], cursor_of: $cur:expr } )+) => {
        $(
            pub struct $name;
            impl Paged for $name {
                const VERB: &'static str = $verb;
                const KEY: &'static str = $key;
                const DEFAULT_LIMIT: u32 = $def;
                const MAX_LIMIT: u32 = $max;
                type Row = $row;
                type Order = $ord;
                const FIELDS: &'static [&'static str] = &[$( $field ),+];
                fn cursor_of(row: &Self::Row, order: Self::Order) -> Cursor { ($cur)(row, order) }
            }
        )+
        /// Every canonical verb that derives [`Paged`], generated from the
        /// one [`paged_verbs!`] list.
        pub const PAGED_VERBS: &[&str] = &[$( $verb ),+];
        const PAGED_SPECS: &[PagedSpec] = &[$(
            PagedSpec { verb: $verb, key: $key, default_limit: $def, max_limit: $max,
                        orders: <$ord as OrderEnum>::names,
                        default_order: || <$ord as OrderEnum>::DEFAULT.name(),
                        fields: <$name as Paged>::FIELDS,
                        input_properties: <$name as Paged>::input_properties }
        ),+];
    };
}

paged_verbs! {
    ChannelRead { verb: "host.channel.read", key: "posts", default: 50, max: 100,
        row: crate::db::ChannelPostRow, order: ChannelOrder,
        fields: ["post_id", "channel_id", "seq", "from_address", "body", "data", "created_at"],
        cursor_of: |p: &crate::db::ChannelPostRow, _o: ChannelOrder| Cursor::new(p.seq, p.id.clone()) }
    MsgInbox { verb: "host.msg.inbox", key: "messages", default: 50, max: 100,
        row: crate::db::MessageRow, order: InboxOrder,
        fields: ["message_id", "thread_id", "seq", "from_address", "body", "data", "in_reply_to", "synthetic", "source_class", "created_at", "read_at", "urgent"],
        cursor_of: |m: &crate::db::MessageRow, _o: InboxOrder| Cursor::new(m.created_unix_ms, m.id.clone()) }
    MsgThread { verb: "host.msg.thread", key: "messages", default: 50, max: 100,
        row: crate::db::MessageRow, order: ThreadOrder,
        fields: ["message_id", "thread_id", "seq", "from_address", "body", "data", "in_reply_to", "synthetic", "source_class", "created_at", "read_at", "urgent"],
        cursor_of: |m: &crate::db::MessageRow, _o: ThreadOrder| Cursor::new(m.seq, m.id.clone()) }
    RunsList { verb: "host.runs.list", key: "runs", default: 20, max: 200,
        row: crate::db::RunPageRow, order: RunsOrder,
        fields: ["run_id", "tool", "trigger", "trigger_ref", "status", "progress", "counters", "result", "result_ref", "purged", "error_class", "error", "started_unix", "finished_unix", "duration_ms", "deadline_s", "attempt", "manual", "test", "end_user", "parent_run_id", "step_no", "parent_tool", "children"],
        cursor_of: |r: &crate::db::RunPageRow, _o: RunsOrder| Cursor::new(r.rowid, r.run.id.clone()) }
    CallsList { verb: "host.calls.list", key: "calls", default: 100, max: 500,
        row: crate::calllog::CallRow, order: CallsOrder,
        fields: ["ts", "credential", "key_label", "tool", "outcome", "error_class", "duration_ms", "bytes_in", "bytes_out", "end_user_subject", "run_id"],
        cursor_of: |r: &crate::calllog::CallRow, _o: CallsOrder| Cursor::new(r.id, r.id.to_string()) }
    DocsList { verb: "host.docs.list", key: "documents", default: 100, max: 1000,
        row: crate::db::DocumentRow, order: DocsOrder,
        fields: ["id", "name", "version", "mime", "bytes", "text_bytes", "metadata", "seq", "created_at", "updated_at", "deleted"],
        cursor_of: |d: &crate::db::DocumentRow, o: DocsOrder| match o {
            DocsOrder::NameAsc => Cursor::new(0, d.name.clone()),
            DocsOrder::CreatedDesc | DocsOrder::CreatedAsc => Cursor::new(d.seq, d.id.clone()),
        } }
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
const UNPAGED_NOTES: &str =
    "newest-first window; its cursor is the previous page's last note key, not a signed keyset cursor";
const UNPAGED_EXPORT: &str =
    "export or arbitrary-query semantics: the caller bounds the result, so no page cursor applies";
const UNPAGED_BOUNDED: &str =
    "tenant-quota-bounded set returned whole in one response, never more than a plan-capped few hundred rows";
const UNPAGED_NAME_WINDOW: &str =
    "limit-only name-ordered window with a prefix filter; no keyset cursor is offered yet";
const UNPAGED_RANKED: &str =
    "relevance-ranked top-k result, not a keyset-ordered list, so there is no stable cursor";
const UNPAGED_REGISTRY: &str =
    "fixed set read whole from providers.toml at build time, a handful of rows; no cursor applies";
const UNPAGED_ECHO: &str =
    "not a list verb: the array is a field of a single-record or mutation response";
const UNPAGED_STATIC: &str =
    "static registry returned whole in one response, bounded by the code table, so no page cursor applies";
const UNPAGED_SUMMARY: &str =
    "one byte-budgeted summary (resume::ResumeBudget): every section is cut to its cap and truncated names the cut, so no page cursor applies";
const UNPAGED_WAIT: &str =
    "long-poll batch of at most 50 rows that advances the shared cursor; wait semantics, not page-through";

pub const UNPAGED_VERBS: &[(&str, &str)] = &[
    ("billing.plans", UNPAGED_ECHO),
    ("host.agent.contacts", UNPAGED_BOUNDED),
    ("host.agent.profile_set", UNPAGED_ECHO),
    ("host.agent.search", UNPAGED_LEGACY),
    ("host.ask.list", UNPAGED_WINDOW),
    ("host.notify.channel_list", UNPAGED_BOUNDED),
    ("host.notify.receipts", UNPAGED_WINDOW),
    ("host.email.sends", UNPAGED_WINDOW),
    ("host.audit.chain", UNPAGED_WINDOW),
    ("host.audit.export", UNPAGED_EXPORT),
    ("host.blob.gc", UNPAGED_BOUNDED),
    ("host.blob.list", UNPAGED_NAME_WINDOW),
    ("host.catalog.search", UNPAGED_WINDOW),
    ("host.changelog", UNPAGED_EXPORT),
    ("host.connect.list", UNPAGED_REGISTRY),
    ("host.connect.providers", UNPAGED_REGISTRY),
    ("host.sync.list", UNPAGED_BOUNDED),
    ("host.docs.search", UNPAGED_RANKED),
    ("host.doctor", UNPAGED_WINDOW),
    ("host.schedule_receipts", UNPAGED_WINDOW),
    ("host.drift.reviews", UNPAGED_WINDOW),
    ("host.enduser.audit", UNPAGED_LEGACY),
    ("host.enduser.list", UNPAGED_LEGACY),
    ("host.example.list", UNPAGED_BOUNDED),
    ("host.errors", UNPAGED_STATIC),
    ("host.feedback.list", UNPAGED_BOUNDED),
    ("host.group.list", UNPAGED_BOUNDED),
    ("host.help.search", UNPAGED_RANKED),
    ("host.index.list", UNPAGED_BOUNDED),
    ("host.index.query", UNPAGED_RANKED),
    ("host.invite.create", UNPAGED_ECHO),
    ("host.invite.list", UNPAGED_BOUNDED),
    ("host.key.list", UNPAGED_BOUNDED),
    ("host.msg.wait", UNPAGED_WAIT),
    ("host.note.list", UNPAGED_NOTES),
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
    ("host.task.dead_letters", UNPAGED_BOUNDED),
    ("host.tool.list", UNPAGED_BOUNDED),
    ("host.tool.list_shared", UNPAGED_WINDOW),
    ("host.trigger.dead_letters", UNPAGED_BOUNDED),
    ("host.trigger.list", UNPAGED_BOUNDED),
    ("host.vault.providers", UNPAGED_BOUNDED),
    ("host.whoami", UNPAGED_ECHO),
    ("host.resume", UNPAGED_SUMMARY),
];


/// The `unpaged_reason` for `verb`, if excused.
pub fn unpaged_reason(verb: &str) -> Option<&'static str> {
    UNPAGED_VERBS.iter().find(|(v, _)| *v == verb).map(|(_, r)| *r)
}

/// Splice the generated paging properties into a verb's input schema
/// (`properties` object), replacing any hand-typed `limit`/`cursor`.
pub fn splice_input_schema(verb: &str, properties: &mut Map<String, Value>) {
    splice_fields_property(verb, properties);
    let Some(spec) = paged_spec(verb) else { return };
    let props = (spec.input_properties)();
    for (k, v) in props {
        properties.insert(k, v);
    }
}

// ---- fields projection ----------------------------------------------------
// PRD-mcphost-fields-projection-on-list-verbs.

/// Row keys of `host.tool.list` (`control::tool_list`). `stub` is present
/// only on an echo-kind tool, the rest always (`forked_from` is `null` on a
/// tool that is not a fork).
pub const TOOL_LIST_FIELDS: &[&str] = &[
    "name", "kind", "created_at", "visibility", "share_description", "unshared_by", "env",
    "advisories", "expose_spec", "exposed_at", "spec_reads", "scopes", "stub", "tags", "tested_version",
    "stale_test", "forked_from",
];

/// List verbs that are not [`Paged`] (no keyset cursor yet) but still accept
/// `fields`: `(verb, array key, row fields)`. They keep their
/// `unpaged_reason`; the only difference from the other excused verbs is that
/// they were asked for a projection explicitly.
pub const PROJECTED_WINDOWS: &[(&str, &str, &[&str])] = &[
    ("host.tool.list", "tools", TOOL_LIST_FIELDS),
];

/// What `paged` owns of a call, registered for the validator (R3). `fields`:
/// `parse_fields` is the one owner of its semantics -- an unknown name is
/// `field_unknown {field, nearest, valid}`, duplicates collapse, an empty or
/// malformed list is `args_invalid` -- and it runs first in the handler, for
/// every verb `fields_for` names. `limit`: above its max it is CLAMPED and
/// reported (`_meta.mcphost.limit_clamped`), never refused, so the published
/// `maximum` documents the clamp and the validator does not enforce it.
pub const ARG_OWNERS: &[crate::host_args::ArgOwner] = &[
    crate::host_args::ArgOwner {
        applies: |tool| fields_for(tool).is_some(),
        path: "/fields",
        keywords: &["*"],
        codes: &["field_unknown", "args_invalid"],
    },
    crate::host_args::ArgOwner {
        applies: |tool| paged_spec(tool).is_some(),
        path: "/limit",
        keywords: &["maximum"],
        codes: &[],
    },
];

/// The set of row fields `verb` accepts in `fields`, `None` for a verb that
/// does not project. Generated into the contract as `fields_enum`.
pub fn fields_for(verb: &str) -> Option<&'static [&'static str]> {
    paged_spec(verb)
        .map(|s| s.fields)
        .or_else(|| PROJECTED_WINDOWS.iter().find(|(v, _, _)| *v == verb).map(|(_, _, f)| *f))
}

/// Every verb that accepts `fields`.
pub fn projected_verbs() -> impl Iterator<Item = &'static str> {
    PAGED_VERBS.iter().copied().chain(PROJECTED_WINDOWS.iter().map(|(v, _, _)| *v))
}

/// The `fields` input property, its `items.enum` being the verb's field set.
pub fn fields_property(fields: &[&str]) -> Value {
    json!({
        "type": "array", "minItems": 1, "uniqueItems": true,
        "items": {"type": "string", "enum": fields},
        "description": FIELDS_DESCRIPTION,
    })
}

/// Add the generated `fields` property to a projecting verb's schema.
pub fn splice_fields_property(verb: &str, properties: &mut Map<String, Value>) {
    if let Some(fields) = fields_for(verb) {
        properties.insert("fields".to_string(), fields_property(fields));
    }
}

/// The closest entry of `valid` to `field` (Levenshtein <= 3, first wins a tie).
pub fn nearest_field(field: &str, valid: &[&str]) -> Option<String> {
    let lower = field.to_ascii_lowercase();
    valid
        .iter()
        .map(|c| (crate::errors::levenshtein(&lower, c), *c))
        .filter(|(d, _)| *d <= 3)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c.to_string())
}

/// `field_unknown {field, nearest, valid}`.
pub fn field_unknown(verb: &str, field: &str, valid: &[&str]) -> AppError {
    AppError::Structured {
        code: FIELD_UNKNOWN,
        message: format!("unknown field '{field}' for {verb}"),
        data: json!({"field": field, "nearest": nearest_field(field, valid), "valid": valid}),
    }
}

/// Read and validate `fields` before any row is fetched: absent/null ->
/// `None` (all fields); empty or malformed -> `args_invalid`; a name outside
/// the verb's set -> `field_unknown`. Duplicates collapse, order is kept.
pub fn parse_fields(verb: &str, args: &Value) -> Result<Option<Vec<String>>, AppError> {
    let Some(raw) = args.get("fields").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let valid = fields_for(verb).unwrap_or(&[]);
    let list = raw
        .as_array()
        .ok_or_else(|| AppError::InvalidArgs("fields: must be an array of field names".to_string()))?;
    if list.is_empty() {
        return Err(AppError::InvalidArgs(
            "fields: must name at least one field (omit it to get every field)".to_string(),
        ));
    }
    let mut out: Vec<String> = Vec::new();
    for item in list {
        let name = item
            .as_str()
            .ok_or_else(|| AppError::InvalidArgs("fields: every entry must be a string".to_string()))?;
        if !valid.contains(&name) {
            return Err(field_unknown(verb, name, valid));
        }
        if !out.iter().any(|f| f == name) {
            out.push(name.to_string());
        }
    }
    Ok(Some(out))
}

/// Keep only `fields` on every row of `page[key]` (after the fetch, so the
/// keyset cursor is minted from the full row).
pub fn project_page(page: &mut Value, key: &str, fields: &[String]) {
    if let Some(rows) = page.get_mut(key).and_then(Value::as_array_mut) {
        for row in rows {
            if let Some(obj) = row.as_object_mut() {
                obj.retain(|k, _| fields.iter().any(|f| f == k));
            }
        }
    }
}

/// `fields` on a verb that has `unpaged_reason` and no projection is
/// `args_invalid` naming the argument, not silently ignored.
pub fn refuse_fields_on_unprojected(verb: &str, args: &Value) -> Result<(), AppError> {
    if args.get("fields").is_some_and(|v| !v.is_null())
        && fields_for(verb).is_none()
        && unpaged_reason(verb).is_some()
    {
        return Err(AppError::InvalidArgs(format!(
            "fields: {verb} returns unpaged rows and does not project; omit fields"
        )));
    }
    Ok(())
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

crate::err_code! {
    pub CURSOR_INVALID,
    code: "cursor_invalid", family: Args, class: AgentFault, retryable: false,
    remedy: crate::paged::CURSOR_REMEDY,
}

crate::err_code! {
    pub FIELD_UNKNOWN,
    code: "field_unknown", family: Args, class: AgentFault, retryable: false,
    remedy: "Use a field from data.valid; data.nearest is the closest match.",
}

