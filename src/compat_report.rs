//! PRD-mcphost-migration-refusal-contract: `mcphost migrate --check-compat
//! --report <path>` writes this document instead of leaving the operator one
//! line (`check-compat: FAILED at step '<step>': <detail>`).
//!
//! Everything a refusal says is DERIVED here, never written next to the
//! deploy tool as prose: the JSON schema (`docs/compat-report.schema.json`)
//! is rendered from [`CompatReport`] by `mcphost migrate
//! --print-report-schema`; the migration list comes from the index
//! `build.rs` generates out of `migrations/*.sql` (name + `-- compat:`
//! header); `reason`/`remedy` come from the one [`REFUSAL_TABLE`] keyed by
//! `(step, compat)`; the exit code comes from [`exit_code`].
//!
//! Migrations in this repo are idempotent Rust-gated batches with no version
//! table, so "the migrations applied beyond the previous release's level" is
//! not recorded anywhere. [`attribute_migrations`] names, instead, every
//! migration whose SQL mentions a table the schema diff changed (and, when
//! the change is column-level, one of its columns): the migrations that
//! explain the diff. A migration applied from an extra directory (the
//! fixture seam) is always listed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/migrations_index.rs"));

/// The one environment variable that lifts a refusal (one-off, set by Joe).
pub const BYPASS_ENV: &str = "MCPHOST_DEPLOY_AUTHORIZE";

/// Exit codes of `--check-compat --report` (R2). Without `--report` the
/// historic exit 4 on any failure is unchanged.
pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_INCOMPATIBLE: i32 = 3;
pub const EXIT_PREVIOUS_UP: i32 = 4;
pub const EXIT_REPORT_UNWRITABLE: i32 = 5;

/// The `-- compat:` header of a migration file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Compat {
    Previous,
    Breaking,
}

impl Compat {
    pub fn as_str(self) -> &'static str {
        match self {
            Compat::Previous => "previous",
            Compat::Breaking => "breaking",
        }
    }

    /// Parse the first non-blank line of a migration file; anything but
    /// `-- compat: previous` reads as `breaking` (the fail-safe reading,
    /// same as `build.rs`).
    pub fn from_header(sql: &str) -> Compat {
        let first = sql.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start();
        if first.starts_with("-- compat: previous") {
            Compat::Previous
        } else {
            Compat::Breaking
        }
    }

    fn from_index_word(word: &str) -> Compat {
        if word == "previous" { Compat::Previous } else { Compat::Breaking }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MigrationRef {
    pub name: String,
    pub compat: Compat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TableChangeKind {
    Added,
    Dropped,
    ColumnsAdded,
    ColumnsDropped,
    Altered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TableChange {
    pub table: String,
    pub change: TableChangeKind,
    pub columns: Vec<String>,
}

/// The single lift: `<env>=<value>` where value is `<from>..<to>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bypass {
    pub env: String,
    pub value: String,
}

/// The document `--report` writes. `step`, `detail`, `reason` and `remedy`
/// are `null` on `ok: true`, and `bypass` is then absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompatReport {
    pub schema_version: u32,
    pub ok: bool,
    /// The previous binary's `--version`.
    pub from: String,
    /// This binary's version.
    pub to: String,
    pub db_path: String,
    pub checked_at_unix: i64,
    pub migrations: Vec<MigrationRef>,
    pub tables_changed: Vec<TableChange>,
    pub step: Option<String>,
    pub detail: Option<String>,
    pub reason: Option<String>,
    pub remedy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass: Option<Bypass>,
}

pub const SCHEMA_VERSION: u32 = 1;

/// The committed `docs/compat-report.schema.json`, byte for byte: pretty
/// JSON with a trailing newline. `mcphost migrate --print-report-schema`
/// prints exactly this.
pub fn report_schema_json() -> String {
    let schema = schemars::schema_for!(CompatReport);
    let mut text = serde_json::to_string_pretty(&schema).unwrap_or_default();
    text.push('\n');
    text
}

/// Every step name `compat_check` can fail at.
pub const STEPS: [&str; 9] = [
    "copy",
    "migrate",
    "spawn",
    "previous-up",
    "initialize",
    "tools_list",
    "signup",
    "tools_call",
    "healthz",
];

/// R5: `(step, compat, reason, remedy)`. `{bypass}` in a remedy is replaced
/// by `<env>=<value>`. A `breaking` remedy names `mcphost-deploy backup`
/// first and the bypass second; no remedy names a file path.
pub const REFUSAL_TABLE: &[(&str, &str, &str, &str)] = &[
    ("copy", "previous", "The live database could not be copied for the check.", "Check that the live database exists and is readable by the deploy user, then rerun the check."),
    ("copy", "breaking", "The live database could not be copied for the check.", "Take a backup with mcphost-deploy backup, fix read access to the live database, then rerun the check."),
    ("migrate", "previous", "This release could not apply its pending migrations to the copy.", "Fix the failing migration in this release and redeploy; the bypass does not apply because this release cannot run its own schema."),
    ("migrate", "breaking", "This release could not apply its pending migrations to the copy.", "Take a backup with mcphost-deploy backup, then fix the failing migration in this release and redeploy; the bypass does not apply because this release cannot run its own schema."),
    ("spawn", "previous", "The previous release could not be launched.", "Point the check at a working previous mcphost binary and rerun."),
    ("spawn", "breaking", "The previous release could not be launched.", "Take a backup with mcphost-deploy backup, point the check at a working previous mcphost binary and rerun."),
    ("previous-up", "previous", "The previous release exited before it answered healthz on the migrated schema.", "Read the detail field for its stderr; if the previous release cannot start for a reason unrelated to the schema, repair it and rerun the check."),
    ("previous-up", "breaking", "The previous release exited before it answered healthz on the migrated schema.", "Take a backup with mcphost-deploy backup, then rerun the deploy with {bypass} to accept that the previous release cannot start on the migrated schema."),
    ("initialize", "previous", "The previous release failed the MCP initialize handshake on the migrated schema.", "No migration is marked breaking: make the migration additive or mark it compat breaking with its restore note, then rebuild."),
    ("initialize", "breaking", "The previous release failed the MCP initialize handshake on the migrated schema.", "Take a backup with mcphost-deploy backup, then rerun the deploy with {bypass}."),
    ("tools_list", "previous", "The previous release failed tools/list on the migrated schema.", "No migration is marked breaking: make the migration additive or mark it compat breaking with its restore note, then rebuild."),
    ("tools_list", "breaking", "The previous release failed tools/list on the migrated schema.", "Take a backup with mcphost-deploy backup, then rerun the deploy with {bypass}."),
    ("signup", "previous", "The previous release failed the throwaway signup on the migrated schema.", "No migration is marked breaking: make the migration additive or mark it compat breaking with its restore note, then rebuild."),
    ("signup", "breaking", "The previous release failed the throwaway signup on the migrated schema.", "Take a backup with mcphost-deploy backup, then rerun the deploy with {bypass}."),
    ("tools_call", "previous", "The previous release failed a control-plane tools/call on the migrated schema.", "No migration is marked breaking: make the migration additive or mark it compat breaking with its restore note, then rebuild."),
    ("tools_call", "breaking", "The previous release failed a control-plane tools/call on the migrated schema.", "Take a backup with mcphost-deploy backup, then rerun the deploy with {bypass}."),
    ("healthz", "previous", "The previous release failed the final healthz probe on the migrated schema.", "No migration is marked breaking: make the migration additive or mark it compat breaking with its restore note, then rebuild."),
    ("healthz", "breaking", "The previous release failed the final healthz probe on the migrated schema.", "Take a backup with mcphost-deploy backup, then rerun the deploy with {bypass}."),
];

/// `(reason, remedy)` for a failing `step` under `compat`, the remedy with
/// `{bypass}` filled in. Unknown steps (a new step added without a table
/// row) get the `breaking` generic text instead of a panic;
/// `compat_report_table_covers_every_step` keeps that case from shipping.
pub fn refusal_text(step: &str, compat: Compat, bypass: &Bypass) -> (String, String) {
    let row = REFUSAL_TABLE
        .iter()
        .find(|(s, c, _, _)| *s == step && *c == compat.as_str());
    let (reason, remedy) = match row {
        Some((_, _, reason, remedy)) => (*reason, *remedy),
        None => (
            "The check failed at a step this release has no refusal text for.",
            "Take a backup with mcphost-deploy backup, then rerun the deploy with {bypass}.",
        ),
    };
    let lift = format!("{}={}", bypass.env, bypass.value);
    (reason.to_string(), remedy.replace("{bypass}", &lift))
}

/// The exit code for a finished report: 0 ok, 4 the previous binary never
/// came up, 3 anything else incompatible. (5, an unwritable report, is
/// decided by the caller that tried to write it.)
pub fn exit_code(report: &CompatReport) -> i32 {
    match (report.ok, report.step.as_deref()) {
        (true, _) => EXIT_OK,
        (false, Some("previous-up")) => EXIT_PREVIOUS_UP,
        (false, _) => EXIT_INCOMPATIBLE,
    }
}

/// The word in `check-compat: <word> report=<path>`.
pub fn outcome_word(report: &CompatReport) -> &'static str {
    match exit_code(report) {
        EXIT_OK => "ok",
        EXIT_PREVIOUS_UP => "previous-up-failed",
        _ => "incompatible",
    }
}

/// One column as `PRAGMA table_info` reports it: name, declared type,
/// not-null, default, primary-key position.
type ColumnDef = (String, String, bool, Option<String>, i64);

fn read_tables(conn: &Connection) -> rusqlite::Result<BTreeMap<String, Vec<ColumnDef>>> {
    let mut names_stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let names: Vec<String> = names_stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut tables = BTreeMap::new();
    for name in names {
        let mut stmt = conn.prepare(
            "SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1) ORDER BY cid",
        )?;
        let columns: Vec<ColumnDef> = stmt
            .query_map([&name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        tables.insert(name, columns);
    }
    Ok(tables)
}

fn names_of(columns: &[ColumnDef]) -> Vec<String> {
    columns.iter().map(|c| c.0.clone()).collect()
}

fn diff_one(table: &str, live: &[ColumnDef], scratch: &[ColumnDef]) -> Option<TableChange> {
    let added: Vec<String> = scratch
        .iter()
        .filter(|c| !live.iter().any(|l| l.0 == c.0))
        .map(|c| c.0.clone())
        .collect();
    let dropped: Vec<String> = live
        .iter()
        .filter(|c| !scratch.iter().any(|s| s.0 == c.0))
        .map(|c| c.0.clone())
        .collect();
    let changed: Vec<String> = scratch
        .iter()
        .filter(|c| live.iter().any(|l| l.0 == c.0 && l != *c))
        .map(|c| c.0.clone())
        .collect();
    let (change, columns) = match (added.is_empty(), dropped.is_empty(), changed.is_empty()) {
        (true, true, true) => return None,
        (false, true, true) => (TableChangeKind::ColumnsAdded, added),
        (true, false, true) => (TableChangeKind::ColumnsDropped, dropped),
        _ => {
            let all: BTreeSet<String> = added.into_iter().chain(dropped).chain(changed).collect();
            (TableChangeKind::Altered, all.into_iter().collect())
        }
    };
    Some(TableChange { table: table.to_string(), change, columns })
}

/// R3: diff `sqlite_master` + `PRAGMA table_info` between the live file
/// (opened read-only) and the migrated scratch copy. Sorted by table name.
pub fn diff_schemas(live_db: &Path, scratch_db: &Path) -> Result<Vec<TableChange>, String> {
    let open = |p: &Path| {
        Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("cannot open {} read-only: {e}", p.display()))
    };
    let live = read_tables(&open(live_db)?).map_err(|e| format!("reading the live schema: {e}"))?;
    let scratch =
        read_tables(&open(scratch_db)?).map_err(|e| format!("reading the scratch schema: {e}"))?;
    let mut changes = Vec::new();
    let names: BTreeSet<&String> = live.keys().chain(scratch.keys()).collect();
    for name in names {
        match (live.get(name), scratch.get(name)) {
            (None, Some(cols)) => changes.push(TableChange {
                table: name.clone(),
                change: TableChangeKind::Added,
                columns: names_of(cols),
            }),
            (Some(cols), None) => changes.push(TableChange {
                table: name.clone(),
                change: TableChangeKind::Dropped,
                columns: names_of(cols),
            }),
            (Some(l), Some(s)) => changes.extend(diff_one(name, l, s)),
            (None, None) => {}
        }
    }
    Ok(changes)
}

/// Lowercased identifier-ish words of `sql` with `--` comment text removed.
fn sql_words(sql: &str) -> BTreeSet<String> {
    sql.lines()
        .map(|l| l.split("--").next().unwrap_or(""))
        .flat_map(|l| l.split(|c: char| !(c.is_alphanumeric() || c == '_')))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A migration the report may name: `(file name, compat header, sql text)`.
pub type MigrationSource<'a> = (&'a str, Compat, &'a str);

/// Every embedded migration (from `build.rs`'s index) whose SQL explains a
/// change in `changes`: it mentions the changed table and, for a
/// column-level change, at least one of the changed columns. `extra` are
/// migrations applied from the fixture directory; those are always named.
pub fn attribute_migrations(
    changes: &[TableChange],
    extra: &[MigrationSource<'_>],
) -> Vec<MigrationRef> {
    let mut out: Vec<MigrationRef> = Vec::new();
    for (name, word, sql) in MIGRATION_INDEX {
        let words = sql_words(sql);
        let explains = changes.iter().any(|c| {
            words.contains(&c.table.to_lowercase())
                && (c.columns.is_empty()
                    || matches!(c.change, TableChangeKind::Added | TableChangeKind::Dropped)
                    || c.columns.iter().any(|col| words.contains(&col.to_lowercase())))
        });
        if explains {
            out.push(MigrationRef { name: (*name).to_string(), compat: Compat::from_index_word(word) });
        }
    }
    for (name, compat, _) in extra {
        out.push(MigrationRef { name: (*name).to_string(), compat: *compat });
    }
    out
}

/// The worst compat across the named migrations (`previous` when none).
pub fn worst_compat(migrations: &[MigrationRef]) -> Compat {
    if migrations.iter().any(|m| m.compat == Compat::Breaking) {
        Compat::Breaking
    } else {
        Compat::Previous
    }
}

/// What a finished check knows, ready to become a [`CompatReport`].
pub struct ReportInputs {
    pub from: String,
    pub to: String,
    pub db_path: String,
    pub checked_at_unix: i64,
    pub migrations: Vec<MigrationRef>,
    pub tables_changed: Vec<TableChange>,
    /// `(step, detail)` of the failing step, `None` when the check passed.
    pub failure: Option<(String, String)>,
}

/// Assemble the report: `reason`/`remedy`/`bypass` appear only on failure.
pub fn build_report(inputs: ReportInputs) -> CompatReport {
    let ReportInputs { from, to, db_path, checked_at_unix, migrations, tables_changed, failure } =
        inputs;
    let bypass = Bypass { env: BYPASS_ENV.to_string(), value: format!("{from}..{to}") };
    let (step, detail, reason, remedy, bypass) = match failure {
        None => (None, None, None, None, None),
        Some((step, detail)) => {
            let (reason, remedy) = refusal_text(&step, worst_compat(&migrations), &bypass);
            (Some(step), Some(detail), Some(reason), Some(remedy), Some(bypass))
        }
    };
    CompatReport {
        schema_version: SCHEMA_VERSION,
        ok: step.is_none(),
        from,
        to,
        db_path,
        checked_at_unix,
        migrations,
        tables_changed,
        step,
        detail,
        reason,
        remedy,
        bypass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generated-contract guard: every step literal `compat_check.rs` can
    /// fail with is in [`STEPS`], and every `(step, compat)` pair has a row.
    #[test]
    fn compat_report_table_covers_every_step() {
        let source = include_str!("compat_check.rs");
        for chunk in source.split("step: \"").skip(1) {
            let step = chunk.split('"').next().unwrap_or("");
            assert!(STEPS.contains(&step), "compat_check.rs fails at unlisted step {step:?}");
        }
        for step in STEPS {
            for compat in ["previous", "breaking"] {
                assert!(
                    REFUSAL_TABLE.iter().any(|(s, c, _, _)| *s == step && *c == compat),
                    "no refusal text for ({step}, {compat})"
                );
            }
        }
    }

    #[test]
    fn compat_report_remedies_have_no_file_path_and_order_backup_first() {
        for (step, compat, _, remedy) in REFUSAL_TABLE {
            assert!(!remedy.contains('/') && !remedy.contains("~"), "({step}, {compat}) names a path");
            assert!(!remedy.contains("authorize"), "({step}, {compat}) names the retired file");
            if *compat == "breaking" && remedy.contains("{bypass}") {
                let backup = remedy.find("mcphost-deploy backup").expect("backup named");
                assert!(backup < remedy.find("{bypass}").unwrap_or(0), "({step}) bypass before backup");
            }
        }
    }

    #[test]
    fn compat_report_index_is_the_migrations_dir() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let on_disk = std::fs::read_dir(dir)
            .expect("read migrations")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "sql"))
            .count();
        assert_eq!(MIGRATION_INDEX.len(), on_disk);
    }
}
