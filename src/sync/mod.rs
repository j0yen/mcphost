//! PRD-mcphost-sync-source-registry-and-create: `host.sync.create {source:
//! {kind, ref}, table, mode?, every?}` registers a scheduled mirror of an
//! external dataset into a host table and returns at once;
//! `host.sync.list/get/delete/run_now` complete the surface. This PRD stores
//! the sync, validates the source, schedules it and enforces `syncs_max`;
//! the pull itself (change detection, rows) is the next PRD, so
//! [`run`] only writes a `not_implemented` receipt into `sync_runs`.
//!
//! [`SourceKind`] is the only list of source kinds: the tool schema's
//! `source.kind` enum ([`source_schema`]), the `args_invalid` message
//! ([`kinds_message`]), the argument list in `docs/tools.md`, the
//! `## Data sync` section of `www/llms.txt` ([`render_llms_section`]) and the
//! puller dispatch ([`puller_for`], an exhaustive `match`) are all generated
//! or iterated from it, and `tests/syreg_ac02_*` asserts they cannot separate.
//!
//! The `syncs` and `sync_runs` tables live in the main database (migration
//! 0130) rather than a per-tenant file, so the trigger tick's sync pass is one
//! indexed query across tenants instead of opening every tenant database.

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::cron::CronSchedule;
use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::{AppState, new_ulid, now_unix};

/// R2: the `every` a create without one gets.
pub const DEFAULT_EVERY: &str = "*/15 * * * *";
/// R2: the fastest schedule a sync may have, in seconds.
pub const MIN_INTERVAL_S: i64 = 300;
/// Technical considerations: a claim older than this belongs to a crashed tick.
pub const STALE_RUN_S: i64 = 600;
/// Consecutive firings [`validate_every`] inspects for a gap under [`MIN_INTERVAL_S`].
const GAP_SAMPLES: usize = 120;
/// Longest `ref` a create accepts, in bytes.
const REF_MAX_BYTES: usize = 2048;
/// Syncs one tick pass starts, so a backlog never stalls the trigger tick.
const TICK_BATCH: i64 = 100;
/// `status` of a pull sync.
pub const STATUS_ACTIVE: &str = "active";
/// `status` of a `push`/`two_way` sync: accepted, inert until the two-way PRD.
pub const STATUS_PENDING_MODE: &str = "pending_mode";

crate::err_code! {
    pub QUOTA_SYNCS_MAX,
    code: "quota_syncs_max", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Delete a sync with host.sync.delete or move to a plan with a higher syncs_max; data.limit and data.used say where you are.",
    data_keys: &["limit", "used"],
}

crate::err_code! {
    pub CONNECTION_MISSING,
    code: "connection_missing", family: Oauth, class: AgentFault, retryable: false,
    remedy: "Connect the provider named in data.provider with host.connect.start, then create the sync again.",
    data_keys: &["provider", "hint"],
}

crate::err_code! {
    pub SCHEDULE_TOO_FREQUENT,
    code: "schedule_too_frequent", family: Args, class: AgentFault, retryable: false,
    remedy: "Use an every schedule that never fires more often than data.min_seconds.",
    data_keys: &["min_seconds"],
}

// ---- the source-kind registry ------------------------------------------------

/// R1: the only list of source kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    CsvUrl,
    GoogleSheet,
    Airtable,
    NotionDb,
    HttpJson,
}

impl SourceKind {
    pub const ALL: &'static [SourceKind] = &[
        SourceKind::CsvUrl,
        SourceKind::GoogleSheet,
        SourceKind::Airtable,
        SourceKind::NotionDb,
        SourceKind::HttpJson,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            SourceKind::CsvUrl => "csv_url",
            SourceKind::GoogleSheet => "google_sheet",
            SourceKind::Airtable => "airtable",
            SourceKind::NotionDb => "notion_db",
            SourceKind::HttpJson => "http_json",
        }
    }

    pub fn parse(s: &str) -> Option<SourceKind> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|k| k.as_str()).collect()
    }

    /// The upstream provider a tenant must have connected (`host.connect.*`)
    /// before a sync of this kind can be created; `None` for a public url.
    pub const fn needs_connection(self) -> Option<&'static str> {
        match self {
            SourceKind::GoogleSheet => Some("google"),
            SourceKind::Airtable => Some("airtable"),
            SourceKind::NotionDb => Some("notion"),
            SourceKind::CsvUrl | SourceKind::HttpJson => None,
        }
    }

    /// One line: what `ref` must be for this kind.
    pub const fn ref_help(self) -> &'static str {
        match self {
            SourceKind::CsvUrl => "public http(s) URL of a CSV file",
            SourceKind::GoogleSheet => "spreadsheet id, or the sheet's URL",
            SourceKind::Airtable => "base id and table id joined by a slash, appXXXX/tblXXXX",
            SourceKind::NotionDb => "database id",
            SourceKind::HttpJson => "public http(s) URL answering a JSON array of objects",
        }
    }

    /// Whether `ref` is a url the host itself will fetch (so it is host-vetted).
    pub const fn ref_is_url(self) -> bool {
        matches!(self, SourceKind::CsvUrl | SourceKind::HttpJson)
    }
}

/// The puller each kind dispatches to. The pull PRD replaces the bodies the
/// names point at; the `match` is exhaustive, so a kind added to
/// [`SourceKind::ALL`] without an arm here does not compile.
pub const fn puller_for(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::CsvUrl => "csv",
        SourceKind::GoogleSheet => "sheets",
        SourceKind::Airtable => "airtable",
        SourceKind::NotionDb => "notion",
        SourceKind::HttpJson => "json",
    }
}

/// `csv_url, google_sheet, ...` -- the list the `args_invalid` message carries.
pub fn kinds_list() -> String {
    SourceKind::names().join(", ")
}

/// The `args_invalid` message for an unknown (or missing) kind.
pub fn kinds_message() -> String {
    format!("source.kind must be one of: {}", kinds_list())
}

/// R2: how a sync moves data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Pull,
    Push,
    TwoWay,
}

impl Mode {
    pub const ALL: &'static [Mode] = &[Mode::Pull, Mode::Push, Mode::TwoWay];

    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Pull => "pull",
            Mode::Push => "push",
            Mode::TwoWay => "two_way",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Self::ALL.iter().copied().find(|m| m.as_str() == s)
    }

    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|m| m.as_str()).collect()
    }
}

/// The `source` property of `host.sync.create`'s input schema; its `kind`
/// enum is generated from [`SourceKind`].
pub fn source_schema() -> Value {
    json!({
        "type": "object",
        "description": "Where the data lives: {kind, ref, secret?}. ref is what kind names (see the Data sync section of /llms.txt); secret is the NAME of a host.secret that holds the source credential, never the value.",
        "properties": {
            "kind": {"type": "string", "enum": SourceKind::names(), "description": "Which source type."},
            "ref": {"type": "string", "maxLength": REF_MAX_BYTES, "examples": ["https://data.example.com/orders.csv"], "description": "The dataset reference for that kind."},
            "secret": {"type": "string", "description": "Name of a host.secret holding the source credential."},
        },
        "required": ["kind", "ref"],
    })
}

/// `mode` property: generated from [`Mode`].
pub fn mode_schema() -> Value {
    json!({
        "type": "string",
        "enum": Mode::names(),
        "description": format!("Direction (default {}). Only pull runs today; push and two_way are accepted and wait with status pending_mode.", Mode::Pull.as_str()),
    })
}

// ---- generated docs ----------------------------------------------------------

pub const LLMS_SECTION_START: &str = "<!-- sync:start -->";
pub const LLMS_SECTION_END: &str = "<!-- sync:end -->";

/// The `## Data sync` section of `www/llms.txt`, one bullet per [`SourceKind`].
pub fn render_llms_section() -> String {
    let mut out = String::new();
    out.push_str(LLMS_SECTION_START);
    out.push_str("\n## Data sync\n\n");
    out.push_str(&format!(
        "Generated by `mcphost gen-docs` from `src/sync/mod.rs` -- do not hand-edit. \
         `host.sync.create` mirrors an external dataset into a host table on a schedule \
         (`every`, default `{DEFAULT_EVERY}`, never faster than {MIN_INTERVAL_S} s); a free tenant \
         holds `syncs_max` of them (`host.quickstart` `limits.plan`). `source.kind` is one of:\n\n"
    ));
    for k in SourceKind::ALL {
        let conn = if k.needs_connection().is_some() {
            "needs a connected account (host.connect.start)"
        } else {
            "no connection needed"
        };
        out.push_str(&format!("- `{}` -- ref is a {}; {conn}\n", k.as_str(), k.ref_help()));
    }
    out.push_str(LLMS_SECTION_END);
    out
}

/// Replace the marked section, or on a first run insert it right before the
/// `## Full documentation` heading (appending if there is none).
pub fn splice_llms_section(content: &str, section: &str) -> String {
    let section = section.trim_end();
    if let (Some(start), Some(end_pos)) = (content.find(LLMS_SECTION_START), content.find(LLMS_SECTION_END)) {
        let end = end_pos + LLMS_SECTION_END.len();
        format!("{}{}{}", &content[..start], section, &content[end..])
    } else if let Some(idx) = content.find("## Full documentation") {
        format!("{}{}\n\n{}", &content[..idx], section, &content[idx..])
    } else {
        format!("{}\n\n{}\n", content.trim_end(), section)
    }
}

// ---- errors ------------------------------------------------------------------

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::InvalidArgs(msg.into())
}

fn not_found(sync_id: &str) -> AppError {
    AppError::Structured {
        code: crate::subkeys::NOT_FOUND,
        message: format!("no sync '{sync_id}' for this tenant"),
        data: json!({"sync_id": sync_id}),
    }
}

fn host_not_allowed(host: &str) -> AppError {
    AppError::Structured {
        code: crate::uptime::HOST_NOT_ALLOWED,
        message: format!("host '{host}' is not allowed as a sync source"),
        data: json!({"field": "source.ref", "host": host}),
    }
}

// ---- request validation ------------------------------------------------------

/// A validated create request.
struct Request {
    kind: SourceKind,
    reference: String,
    secret_name: Option<String>,
    table: String,
    mode: Mode,
    every: String,
    primary_key: Option<String>,
}

fn opt_str<'a>(obj: &'a serde_json::Map<String, Value>, key: &str, what: &str) -> Result<Option<&'a str>, AppError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(invalid(format!("{what} must be a string"))),
    }
}

fn parse_request(args: &Value) -> Result<Request, AppError> {
    let top = args.as_object().ok_or_else(|| invalid("arguments must be an object"))?;
    let source = top
        .get("source")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("missing required argument 'source' ({kind, ref, secret?})"))?;
    let kind = opt_str(source, "kind", "source.kind")?
        .and_then(SourceKind::parse)
        .ok_or_else(|| invalid(kinds_message()))?;
    let reference = opt_str(source, "ref", "source.ref")?
        .filter(|r| !r.trim().is_empty())
        .ok_or_else(|| invalid("missing required argument 'source.ref'"))?;
    if reference.len() > REF_MAX_BYTES {
        return Err(invalid(format!("source.ref is {} bytes; the limit is {REF_MAX_BYTES}", reference.len())));
    }
    let secret_name = opt_str(source, "secret", "source.secret")?.map(str::to_string);
    if let Some(name) = &secret_name
        && (name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
    {
        return Err(invalid("source.secret must be the name of a host.secret (letters, digits, _ - ., at most 64)"));
    }
    let table = opt_str(top, "table", "table")?
        .ok_or_else(|| invalid("missing required argument 'table'"))?
        .to_string();
    if !crate::tables::is_valid_ident(&table) {
        return Err(invalid(format!("table: must match {}; got '{table}'", crate::tables::IDENT_PATTERN)));
    }
    let mode = match opt_str(top, "mode", "mode")? {
        None => Mode::Pull,
        Some(m) => Mode::parse(m).ok_or_else(|| invalid(format!("mode must be one of: {}", Mode::names().join(", "))))?,
    };
    let every = opt_str(top, "every", "every")?.unwrap_or(DEFAULT_EVERY).trim().to_string();
    let primary_key = opt_str(top, "primary_key", "primary_key")?.map(str::to_string);
    if let Some(pk) = &primary_key
        && !crate::tables::is_valid_ident(pk)
    {
        return Err(invalid(format!("primary_key: must match {}; got '{pk}'", crate::tables::IDENT_PATTERN)));
    }
    Ok(Request { kind, reference: reference.to_string(), secret_name, table, mode, every, primary_key })
}

/// R2: parses `every` and refuses a schedule that fires faster than
/// [`MIN_INTERVAL_S`]. Returns the first firing after `now`.
fn validate_every(every: &str, now: i64) -> Result<i64, AppError> {
    let schedule = CronSchedule::parse(every).map_err(|e| invalid(format!("every: {}", e.message)))?;
    let first = schedule
        .next_after(now)
        .ok_or_else(|| invalid("every: the schedule never fires"))?;
    let mut prev = first;
    for _ in 0..GAP_SAMPLES {
        let Some(next) = schedule.next_after(prev) else { break };
        if next - prev < MIN_INTERVAL_S {
            return Err(AppError::Structured {
                code: SCHEDULE_TOO_FREQUENT,
                message: format!("every '{every}' fires faster than every {MIN_INTERVAL_S} seconds"),
                data: json!({"min_seconds": MIN_INTERVAL_S}),
            });
        }
        prev = next;
    }
    Ok(first)
}

/// R3: a url ref must be an http(s) url on a public host.
fn check_url_ref(state: &AppState, reference: &str) -> Result<(), AppError> {
    let url = reqwest::Url::parse(reference).map_err(|e| invalid(format!("source.ref is not a valid url: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("source.ref must be an http or https url"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| invalid("source.ref has no host"))?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let (own_domain, allow_loopback) = crate::index::url_policy(state);
    if crate::kinds::http::is_disallowed_literal_host(host, &own_domain, allow_loopback) {
        return Err(host_not_allowed(host));
    }
    Ok(())
}

/// R3: an OAuth kind needs a live tenant-level connection for its provider.
async fn check_connection(state: &AppState, tenant: &Tenant, kind: SourceKind) -> Result<(), AppError> {
    let Some(provider) = kind.needs_connection() else { return Ok(()) };
    let token = state
        .db
        .get_vault_token(tenant.id, provider.to_string(), crate::connect::flow::TENANT_SUBJECT.to_string())
        .await?;
    if token.is_some_and(|t| t.revoked_unix.is_none()) {
        return Ok(());
    }
    Err(AppError::Structured {
        code: CONNECTION_MISSING,
        message: format!("no connected '{provider}' for this tenant"),
        data: json!({"provider": provider, "hint": format!("call host.connect.start(provider={provider})")}),
    })
}

// ---- rows ----------------------------------------------------------------------

struct SyncRow {
    id: String,
    kind: String,
    reference: String,
    secret_name: Option<String>,
    table: String,
    mode: String,
    schedule: String,
    primary_key: Option<String>,
    status: String,
    created_unix: i64,
    next_run_unix: i64,
}

impl SyncRow {
    const COLUMNS: &'static str =
        "id, kind, ref, secret_name, table_name, mode, schedule, primary_key, status, created_unix, next_run_unix";

    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SyncRow> {
        Ok(SyncRow {
            id: r.get(0)?,
            kind: r.get(1)?,
            reference: r.get(2)?,
            secret_name: r.get(3)?,
            table: r.get(4)?,
            mode: r.get(5)?,
            schedule: r.get(6)?,
            primary_key: r.get(7)?,
            status: r.get(8)?,
            created_unix: r.get(9)?,
            next_run_unix: r.get(10)?,
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "sync_id": self.id,
            "kind": self.kind,
            "ref": self.reference,
            "secret": self.secret_name,
            "table": self.table,
            "mode": self.mode,
            "every": self.schedule,
            "primary_key": self.primary_key,
            "status": self.status,
            "created_unix": self.created_unix,
            "next_run_unix": self.next_run_unix,
        })
    }
}

fn plan_syncs_max(state: &AppState, tenant: &Tenant) -> Result<i64, AppError> {
    state
        .plans
        .get(&tenant.plan)
        .map(|p| p.syncs_max)
        .ok_or_else(|| AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan)))
}

fn quota_syncs_max(limit: i64, used: i64) -> AppError {
    AppError::Structured {
        code: QUOTA_SYNCS_MAX,
        message: format!("sync quota exceeded: syncs_max (limit {limit}, used {used})"),
        data: json!({"quota": "syncs_max", "limit": limit, "used": used}),
    }
}

async fn find_existing(state: &AppState, tenant_id: i64, req: &Request) -> Result<Option<SyncRow>, AppError> {
    let (kind, reference, table) = (req.kind.as_str().to_string(), req.reference.clone(), req.table.clone());
    state
        .db
        .with_conn(move |conn| {
            conn.query_row(
                &format!(
                    "SELECT {} FROM syncs WHERE tenant_id = ?1 AND kind = ?2 AND ref = ?3 AND table_name = ?4",
                    SyncRow::COLUMNS
                ),
                params![tenant_id, kind, reference, table],
                SyncRow::from_row,
            )
            .optional()
            .map_err(AppError::from)
        })
        .await
}

async fn count_syncs(state: &AppState, tenant_id: i64) -> Result<i64, AppError> {
    state
        .db
        .with_conn(move |conn| {
            conn.query_row("SELECT COUNT(*) FROM syncs WHERE tenant_id = ?1", params![tenant_id], |r| r.get(0))
                .map_err(AppError::from)
        })
        .await
}

/// R2: the table the sync mirrors into, created with `host.table.create`
/// semantics when absent. Until the pull PRD infers columns it is text-only:
/// the primary key (when named) or a single `value` column.
async fn ensure_table(state: &AppState, tenant: &Tenant, req: &Request) -> Result<(), AppError> {
    if crate::tables::table_exists(state, tenant, &req.table).await? {
        return Ok(());
    }
    let column = req.primary_key.clone().unwrap_or_else(|| "value".to_string());
    let mut create = json!({"name": req.table, "columns": {column: "text"}});
    if let Some(pk) = &req.primary_key {
        create["primary_key"] = json!(pk);
    }
    crate::tables::table_create(state, tenant, &create).await?;
    Ok(())
}

// ---- host.sync.* ---------------------------------------------------------------

/// `host.sync.create`.
pub async fn create(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let req = parse_request(args)?;
    if req.kind.ref_is_url() {
        check_url_ref(state, &req.reference)?;
    }
    check_connection(state, tenant, req.kind).await?;
    let now = now_unix();
    let next_run_unix = validate_every(&req.every, now)?;

    // R7: the same (kind, ref, table) is the same sync.
    if let Some(row) = find_existing(state, tenant.id, &req).await? {
        let mut out = row.to_json();
        out["existing"] = json!(true);
        return Ok(out);
    }
    let limit = plan_syncs_max(state, tenant)?;
    let used = count_syncs(state, tenant.id).await?;
    if used >= limit {
        return Err(quota_syncs_max(limit, used));
    }
    ensure_table(state, tenant, &req).await?;

    let status = if req.mode == Mode::Pull { STATUS_ACTIVE } else { STATUS_PENDING_MODE };
    let row = SyncRow {
        id: new_ulid(),
        kind: req.kind.as_str().to_string(),
        reference: req.reference.clone(),
        secret_name: req.secret_name.clone(),
        table: req.table.clone(),
        mode: req.mode.as_str().to_string(),
        schedule: req.every.clone(),
        primary_key: req.primary_key.clone(),
        status: status.to_string(),
        created_unix: now,
        next_run_unix,
    };
    let mut out = row.to_json();
    out["existing"] = json!(false);
    let tenant_id = tenant.id;
    let inserted = state
        .db
        .with_conn(move |conn| {
            // The count and the insert share one connection guard, so two
            // concurrent creates cannot both take the last slot.
            let used: i64 =
                conn.query_row("SELECT COUNT(*) FROM syncs WHERE tenant_id = ?1", params![tenant_id], |r| r.get(0))?;
            if used >= limit {
                return Ok(Err(used));
            }
            conn.execute(
                "INSERT INTO syncs (id, tenant_id, kind, ref, secret_name, table_name, mode, schedule, \
                 primary_key, status, created_unix, next_run_unix) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    row.id,
                    tenant_id,
                    row.kind,
                    row.reference,
                    row.secret_name,
                    row.table,
                    row.mode,
                    row.schedule,
                    row.primary_key,
                    row.status,
                    row.created_unix,
                    row.next_run_unix,
                ],
            )?;
            Ok(Ok(()))
        })
        .await?;
    if let Err(used) = inserted {
        return Err(quota_syncs_max(limit, used));
    }
    Ok(out)
}

async fn load(state: &AppState, tenant_id: i64, sync_id: &str) -> Result<SyncRow, AppError> {
    let id = sync_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            conn.query_row(
                &format!("SELECT {} FROM syncs WHERE tenant_id = ?1 AND id = ?2", SyncRow::COLUMNS),
                params![tenant_id, id],
                SyncRow::from_row,
            )
            .optional()
            .map_err(AppError::from)
        })
        .await?
        .ok_or_else(|| not_found(sync_id))
}

fn sync_id_arg(args: &Value) -> Result<&str, AppError> {
    args.get("sync_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("missing required argument 'sync_id'"))
}

/// `host.sync.list`: this tenant's syncs, oldest first.
pub async fn list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let tenant_id = tenant.id;
    let rows = state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {} FROM syncs WHERE tenant_id = ?1 ORDER BY created_unix, id",
                SyncRow::COLUMNS
            ))?;
            let rows = stmt.query_map(params![tenant_id], SyncRow::from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;
    Ok(json!({"syncs": rows.iter().map(SyncRow::to_json).collect::<Vec<_>>()}))
}

/// `host.sync.get {sync_id}`: the row plus its newest `sync_runs` receipt.
pub async fn get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let sync_id = sync_id_arg(args)?;
    let row = load(state, tenant.id, sync_id).await?;
    let id = row.id.clone();
    let last_run = state
        .db
        .with_conn(move |conn| {
            conn.query_row(
                "SELECT id, started_unix, duration_ms, status, rows_added, rows_updated, rows_deleted, bytes, \
                 change_detected, error FROM sync_runs WHERE sync_id = ?1 ORDER BY started_unix DESC, id DESC LIMIT 1",
                params![id],
                |r| {
                    Ok(json!({
                        "run_id": r.get::<_, String>(0)?,
                        "started_unix": r.get::<_, i64>(1)?,
                        "duration_ms": r.get::<_, i64>(2)?,
                        "status": r.get::<_, String>(3)?,
                        "rows_added": r.get::<_, i64>(4)?,
                        "rows_updated": r.get::<_, i64>(5)?,
                        "rows_deleted": r.get::<_, i64>(6)?,
                        "bytes": r.get::<_, i64>(7)?,
                        "change_detected": r.get::<_, i64>(8)? != 0,
                        "error": r.get::<_, Option<String>>(9)?,
                    }))
                },
            )
            .optional()
            .map_err(AppError::from)
        })
        .await?;
    let mut out = row.to_json();
    out["last_run"] = last_run.unwrap_or(Value::Null);
    Ok(out)
}

/// `host.sync.delete {sync_id}`: removes the sync and its receipts; the table
/// and every row in it stay.
pub async fn delete(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let sync_id = sync_id_arg(args)?;
    let row = load(state, tenant.id, sync_id).await?;
    let (tenant_id, id) = (tenant.id, row.id.clone());
    state
        .db
        .with_conn(move |conn| {
            conn.execute("DELETE FROM sync_runs WHERE sync_id = ?1", params![id])?;
            conn.execute("DELETE FROM syncs WHERE tenant_id = ?1 AND id = ?2", params![tenant_id, id])?;
            Ok(())
        })
        .await?;
    Ok(json!({"sync_id": row.id, "deleted": true, "table": row.table, "table_kept": true}))
}

/// `host.sync.run_now {sync_id}`: makes the sync due now; the trigger tick's
/// sync pass picks it up on its next pass.
pub async fn run_now(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let sync_id = sync_id_arg(args)?;
    let row = load(state, tenant.id, sync_id).await?;
    if row.status != STATUS_ACTIVE {
        return Err(invalid(format!(
            "sync '{}' has status {}: mode {} runs once the two-way sync lands",
            row.id, row.status, row.mode
        )));
    }
    let now = now_unix();
    let (tenant_id, id) = (tenant.id, row.id.clone());
    state
        .db
        .with_conn(move |conn| {
            conn.execute("UPDATE syncs SET next_run_unix = ?3 WHERE tenant_id = ?1 AND id = ?2", params![tenant_id, id, now])?;
            Ok(())
        })
        .await?;
    Ok(json!({"sync_id": row.id, "next_run_unix": now}))
}

// ---- the trigger tick's sync pass ------------------------------------------------

/// R6: starts every active sync whose `next_run_unix` has passed. Each one is
/// claimed first (its `next_run_unix` moves to the next cron slot in the same
/// statement), so a second pass in the same slot finds nothing due.
pub async fn tick(state: &AppState, now: i64) -> Result<(), AppError> {
    let stale_before = now - STALE_RUN_S;
    let due: Vec<(String, i64, String)> = state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, next_run_unix, schedule FROM syncs \
                 WHERE status = 'active' AND next_run_unix <= ?1 \
                 AND (running_since_unix IS NULL OR running_since_unix <= ?2) \
                 ORDER BY next_run_unix LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(params![now, stale_before, TICK_BATCH], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await?;
    for (id, old_next, schedule) in due {
        let next = CronSchedule::parse(&schedule)
            .ok()
            .and_then(|s| s.next_after(now))
            .unwrap_or(now + MIN_INTERVAL_S);
        let (claim_id, claim_stale) = (id.clone(), stale_before);
        let claimed = state
            .db
            .with_conn(move |conn| {
                let n = conn.execute(
                    "UPDATE syncs SET next_run_unix = ?2, running_since_unix = ?3 \
                     WHERE id = ?1 AND next_run_unix = ?4 AND status = 'active' \
                     AND (running_since_unix IS NULL OR running_since_unix <= ?5)",
                    params![claim_id, next, now, old_next, claim_stale],
                )?;
                Ok(n == 1)
            })
            .await?;
        if claimed && let Err(e) = run(state, &id, now).await {
            tracing::warn!(error = %e, sync_id = %id, "sync run failed");
        }
    }
    Ok(())
}

/// The pull of one sync. Until the pull PRD replaces the body the dispatched
/// puller is only named: the receipt says `not_implemented`.
pub async fn run(state: &AppState, sync_id: &str, started_unix: i64) -> Result<(), AppError> {
    let id = sync_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            let kind: Option<String> =
                conn.query_row("SELECT kind FROM syncs WHERE id = ?1", params![id], |r| r.get(0)).optional()?;
            let puller = kind.as_deref().and_then(SourceKind::parse).map(puller_for).unwrap_or("unknown");
            conn.execute(
                "INSERT INTO sync_runs (id, sync_id, started_unix, duration_ms, status, error) \
                 VALUES (?1, ?2, ?3, 0, 'not_implemented', ?4)",
                params![new_ulid(), id, started_unix, format!("the {puller} puller lands with the pull PRD")],
            )?;
            conn.execute("UPDATE syncs SET running_since_unix = NULL WHERE id = ?1", params![id])?;
            Ok(())
        })
        .await
}

/// Test support: moves a sync's `next_run_unix`.
#[cfg(feature = "test-support")]
pub async fn set_next_run_for_test(state: &AppState, sync_id: &str, next_run_unix: i64) -> Result<(), AppError> {
    let id = sync_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            conn.execute("UPDATE syncs SET next_run_unix = ?2 WHERE id = ?1", params![id, next_run_unix])?;
            Ok(())
        })
        .await
}

/// Test support: how many `sync_runs` receipts a sync has.
#[cfg(feature = "test-support")]
pub async fn receipt_count_for_test(state: &AppState, sync_id: &str) -> Result<i64, AppError> {
    let id = sync_id.to_string();
    state
        .db
        .with_conn(move |conn| {
            conn.query_row("SELECT COUNT(*) FROM sync_runs WHERE sync_id = ?1", params![id], |r| r.get(0))
                .map_err(AppError::from)
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_their_names() {
        for k in SourceKind::ALL {
            assert_eq!(SourceKind::parse(k.as_str()), Some(*k));
        }
        assert_eq!(SourceKind::parse("excel_online"), None);
    }

    #[test]
    fn every_faster_than_five_minutes_is_refused() {
        assert!(validate_every("*/15 * * * *", 0).is_ok());
        assert!(validate_every("*/5 * * * *", 0).is_ok());
        assert!(validate_every("*/4 * * * *", 0).is_err());
        assert!(validate_every("0,2 * * * *", 0).is_err());
    }
}
