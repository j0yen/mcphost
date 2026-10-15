//! PRD-mcphost-backup-restore-proof-nightly R1/R2/R6: a canonical digest
//! over any copy of the database file, so a restore can be compared with
//! the live file it was taken from.
//!
//! The digested table set is GENERATED from `sqlite_master` at run time
//! minus [`DIGEST_EXCLUDE`]; [`uncovered_tables`] is the drift check a test
//! runs against a freshly migrated db, so a new migration cannot add a
//! table that is neither digested nor named as excluded.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The migration count (highest `migrations/NNNN_*.sql` prefix); a test
/// asserts it cannot drift from the migrations directory.
pub const SCHEMA_VERSION: u32 = 106;

/// `mcphost backup digest` exit code on a failed `PRAGMA integrity_check`.
pub const EXIT_INTEGRITY: i32 = 3;
/// `mcphost backup digest --compare` exit code on mismatching digests.
pub const EXIT_MISMATCH: i32 = 4;

/// High-churn tables left out of the digest: their rows keep growing
/// between the moment a backup is taken and the moment the live file is
/// digested, so counting them would make every honest backup "mismatch".
pub const DIGEST_EXCLUDE: &[&str] = &["calls", "logs", "status_samples", "alerts"];

/// One digest run: the JSON to print and whether the file passed
/// `PRAGMA integrity_check`.
pub struct DigestReport {
    pub value: Value,
    pub integrity_ok: bool,
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Every user table in `sqlite_master` (shadow tables of virtual tables
/// included; `sqlite_*` internals are not user data).
pub fn master_tables(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.collect()
}

/// R2's drift check: the tables in `master` that are in neither `digested`
/// nor `exclude`. Empty means every table is accounted for.
pub fn uncovered_tables(
    master: &[String],
    digested: &BTreeSet<String>,
    exclude: &[&str],
) -> Vec<String> {
    master
        .iter()
        .filter(|t| !digested.contains(*t) && !exclude.contains(&t.as_str()))
        .cloned()
        .collect()
}

fn integrity_text(conn: &Connection) -> rusqlite::Result<String> {
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let lines: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(lines.join("\n"))
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0)
}

fn failed(integrity: String) -> DigestReport {
    DigestReport {
        value: json!({
            "schema_version": SCHEMA_VERSION,
            "integrity": integrity,
            "tables": {},
            "counts": null,
            "digest": null,
        }),
        integrity_ok: false,
    }
}

/// Digest the database file at `path`, opened read-only. Never panics: a
/// file that cannot be opened or whose `integrity_check` is not `ok` yields
/// a report whose `integrity` holds the failure text.
pub fn digest_db(path: &Path, now_unix: i64) -> DigestReport {
    let conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(e) => return failed(e.to_string()),
    };
    let integrity = match integrity_text(&conn) {
        Ok(t) => t,
        Err(e) => return failed(e.to_string()),
    };
    if integrity != "ok" {
        return failed(integrity);
    }
    match digest_tables(&conn, now_unix) {
        Ok(tables_and_counts) => {
            let (tables, counts) = tables_and_counts;
            let canonical = serde_json::to_vec(&json!({
                "schema_version": SCHEMA_VERSION,
                "tables": tables,
                "counts": counts,
            }))
            .unwrap_or_default();
            let digest = format!("{:x}", Sha256::digest(&canonical));
            DigestReport {
                value: json!({
                    "schema_version": SCHEMA_VERSION,
                    "integrity": "ok",
                    "tables": tables,
                    "counts": {
                        "tenants": counts["tenants"],
                        "tools": counts["tools"],
                        "triggers": counts["triggers"],
                        "calls_30d": count(
                            &conn,
                            &format!(
                                "SELECT COUNT(*) FROM calls WHERE started_unix >= {}",
                                now_unix - 30 * 86_400
                            ),
                        ),
                    },
                    "digest": digest,
                }),
                integrity_ok: true,
            }
        }
        Err(e) => failed(e.to_string()),
    }
}

/// One read transaction: per-table `{rows, max_rowid}` for every
/// non-excluded table, plus the stable counts. `calls_30d` is reported but
/// kept out of the digest input (it moves with the clock and with `calls`,
/// which is excluded for the same reason).
fn digest_tables(
    conn: &Connection,
    _now_unix: i64,
) -> rusqlite::Result<(BTreeMap<String, Value>, Value)> {
    conn.execute_batch("BEGIN")?;
    let result = (|| {
        let mut tables = BTreeMap::new();
        for name in master_tables(conn)? {
            if DIGEST_EXCLUDE.contains(&name.as_str()) {
                continue;
            }
            let q = quote_ident(&name);
            let entry = conn
                .query_row(
                    &format!("SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM {q}"),
                    [],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )
                // WITHOUT ROWID tables have no rowid: count only.
                .or_else(|_| {
                    conn.query_row(&format!("SELECT COUNT(*) FROM {q}"), [], |r| {
                        Ok((r.get::<_, i64>(0)?, 0))
                    })
                })?;
            tables.insert(name, json!({ "rows": entry.0, "max_rowid": entry.1 }));
        }
        let counts = json!({
            "tenants": count(conn, "SELECT COUNT(*) FROM tenants"),
            "tools": count(conn, "SELECT COUNT(*) FROM tools"),
            "triggers": count(conn, "SELECT COUNT(*) FROM triggers"),
        });
        Ok((tables, counts))
    })();
    let _ = conn.execute_batch("END");
    result
}

fn table_rows(report: &Value, table: &str) -> String {
    match report["tables"].get(table) {
        Some(t) => t["rows"].to_string(),
        None => "absent".to_string(),
    }
}

/// R6: compare two digest reports. `None` when equal; otherwise a line
/// naming the first differing table (sorted order) with both row counts,
/// then the headline counts. Reports from different `schema_version`s are
/// comparable on `counts` only.
pub fn compare_reports(a: &Value, b: &Value) -> Option<String> {
    let counts_line = format!(
        "counts a={} b={}",
        a["counts"], b["counts"]
    );
    if a["schema_version"] != b["schema_version"] {
        let differ = ["tenants", "tools", "triggers"]
            .iter()
            .find(|k| a["counts"][**k] != b["counts"][**k])?;
        return Some(format!(
            "mismatch: {differ} a={} b={} (schema_version {} vs {}: counts only); {counts_line}",
            a["counts"][*differ], b["counts"][*differ], a["schema_version"], b["schema_version"]
        ));
    }
    if !a["digest"].is_null() && a["digest"] == b["digest"] {
        return None;
    }
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for r in [a, b] {
        if let Some(t) = r["tables"].as_object() {
            names.extend(t.keys().map(String::as_str));
        }
    }
    let differing = names.into_iter().find(|n| a["tables"].get(*n) != b["tables"].get(*n));
    Some(match differing {
        Some(t) => format!(
            "mismatch: {t} rows a={} b={}; {counts_line}",
            table_rows(a, t),
            table_rows(b, t)
        ),
        None => format!("mismatch: digest a={} b={}; {counts_line}", a["digest"], b["digest"]),
    })
}
