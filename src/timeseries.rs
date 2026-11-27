//! `host.ts.*` (PRD-mcphost-timeseries-buckets): numbers with a timestamp go in
//! with `put`, come back bucketed with `get`. One table, `ts_points(series, ts,
//! value, tags_json)`, lives in the tenant's own `tables/<tenant_id>.db` (created
//! lazily on first use, counted toward `table_bytes_max` as part of the file);
//! `get` computes `count/min/max/avg/p50/p95` per bucket in ONE SQL statement
//! (window functions, nearest-rank percentiles) under [`crate::tables::QUERY_TIME_CAP`].
//!
//! The allowed bucket set ([`ALLOWED_BUCKETS`]) and the relative-time grammar
//! ([`TIME_FORMS`]) are one const each: the validator reads them, the tool
//! description renders them, and every refusal carries them (`allowed_buckets`,
//! `time_forms`).

use std::collections::BTreeMap;

use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};
use serde_json::{Map, Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;
use crate::tables::{QUERY_TIME_CAP, arg_str, plan_of, tenant_db_path, with_tenant_conn};

/// The table every series shares.
pub const TS_TABLE: &str = "ts_points";

/// Every bucket width `host.ts.get` accepts, with its width in seconds.
pub const ALLOWED_BUCKETS: &[(&str, i64)] =
    &[("1m", 60), ("5m", 300), ("15m", 900), ("1h", 3_600), ("6h", 21_600), ("1d", 86_400)];

/// Every spelling `from` / `to` accept: unix seconds, `now`, or a negative
/// offset from now. [`parse_time`] is the one reader of this grammar.
pub const TIME_FORMS: &[&str] = &["<unix seconds>", "now", "-<n>s", "-<n>m", "-<n>h", "-<n>d"];

/// `^[a-z][a-z0-9_]{0,40}$`, the descriptor's `series` pattern; [`is_valid_series`]
/// reads the same shape.
pub const SERIES_PATTERN: &str = "^[a-z][a-z0-9_]{0,40}$";

/// The one regex the `from` / `to` descriptors carry: [`TIME_FORMS`] as the
/// schema validator reads it ([`parse_time`] reads the same grammar).
pub const TIME_PATTERN: &str = "^(now|[0-9]+|-[0-9]+[smhd])$";

/// The `data` every refusal of this family carries: the two consts the
/// validator reads, as `allowed_buckets` and `time_forms`. The descriptor
/// embeds the same object (`host_args::REFUSAL_DATA_KEY`), so a refusal from
/// the schema validator and one from the handler agree.
pub fn refusal_data() -> Value {
    json!({"allowed_buckets": allowed_bucket_names(), "time_forms": TIME_FORMS})
}

/// The most tags one point may carry.
pub const TAGS_MAX: usize = 8;

/// The most series one `host.ts.list` names; past it the answer says `truncated: true`.
pub const SERIES_LIST_MAX: i64 = 500;

/// The most buckets one `host.ts.get` answers; a wider range is refused so a
/// reply stays small (narrow the range or pick a wider bucket).
pub const BUCKETS_MAX: i64 = 5_000;

const SERIES_MAX_LEN: usize = 41;
const TAG_KEY_MAX_LEN: usize = 40;
const TAG_VALUE_MAX_LEN: usize = 200;

/// `^[a-z][a-z0-9_]{0,40}$`.
pub fn is_valid_series(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z'))
        && s.len() <= SERIES_MAX_LEN
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
}

/// `1h` -> 3600; `None` for a width outside [`ALLOWED_BUCKETS`].
pub fn bucket_seconds(bucket: &str) -> Option<i64> {
    ALLOWED_BUCKETS.iter().find(|(name, _)| *name == bucket).map(|(_, s)| *s)
}

/// The bucket names, in width order.
pub fn allowed_bucket_names() -> Vec<&'static str> {
    ALLOWED_BUCKETS.iter().map(|(name, _)| *name).collect()
}

/// Parses one `from` / `to` value against [`TIME_FORMS`]: an integer (or an
/// all-digit string) is unix seconds, `now` is `now`, `-<n><s|m|h|d>` is that
/// long before `now`.
pub fn parse_time(v: &Value, now: i64) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().filter(|t| *t >= 0),
        Value::String(s) => {
            if s == "now" {
                return Some(now);
            }
            if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                return s.parse().ok();
            }
            let rest = s.strip_prefix('-')?;
            let (digits, unit) = rest.split_at(rest.len().checked_sub(1)?);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n: i64 = digits.parse().ok()?;
            let unit_s = match unit {
                "s" => 1,
                "m" => 60,
                "h" => 3_600,
                "d" => 86_400,
                _ => return None,
            };
            n.checked_mul(unit_s).map(|d| now - d)
        }
        _ => None,
    }
}

/// Whether `s` is one of the string spellings of [`TIME_FORMS`] ([`TIME_PATTERN`]'s
/// handler twin, for `host_args::SHAPE_CHECKS`).
pub fn is_time_form(s: &str) -> bool {
    parse_time(&Value::String(s.to_string()), 0).is_some()
}

/// `args_invalid` for `field`, rendered through [`crate::kinds::ArgsError`] like
/// every other `args_invalid` site, plus the two consts every refusal of this
/// family names (`allowed_buckets`, `time_forms`).
fn refusal(field: &str, got: String, message: String) -> AppError {
    let args_err = crate::kinds::ArgsError {
        phase: "args_coercion",
        instance_path: format!("/{field}"),
        argument: Some(field.to_string()),
        field: None,
        expected_type: None,
        actual_type: None,
        got,
        docs: "host.ts_get",
        raw: message,
    };
    let mut data = args_err.data();
    if let (Some(obj), Value::Object(extra)) = (data.as_object_mut(), refusal_data()) {
        obj.extend(extra);
    }
    AppError::Structured { code: crate::errors::ARGS_INVALID, message: args_err.message(), data }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn ensure_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {TS_TABLE} (
            series TEXT NOT NULL,
            ts INTEGER NOT NULL,
            value REAL NOT NULL,
            tags_json TEXT
        );
        CREATE INDEX IF NOT EXISTS {TS_TABLE}_series_ts ON {TS_TABLE} (series, ts);"
    ))?;
    Ok(())
}

fn series_arg(args: &Value) -> Result<String, AppError> {
    let series = arg_str(args, "series")?;
    if !is_valid_series(&series) {
        return Err(AppError::InvalidArgs(format!(
            "series: must match ^[a-z][a-z0-9_]{{0,40}}$; got {series:?}"
        )));
    }
    Ok(series)
}

/// `tags` as a sorted string map, at most [`TAGS_MAX`] entries.
fn tags_arg(args: &Value) -> Result<BTreeMap<String, String>, AppError> {
    let mut out = BTreeMap::new();
    let Some(raw) = args.get("tags").filter(|t| !t.is_null()) else {
        return Ok(out);
    };
    let Some(obj) = raw.as_object() else {
        return Err(AppError::InvalidArgs("tags: must be an object of string values".into()));
    };
    // The descriptor states the bound (`maxProperties`); this is the same bound for
    // a call that reaches the handler without the validator (the sandbox bridge).
    if obj.len().saturating_sub(TAGS_MAX) > 0 {
        return Err(AppError::InvalidArgs(format!("tags: at most {TAGS_MAX} tags; got {}", obj.len())));
    }
    for (k, v) in obj {
        let key_ok = !k.is_empty()
            && k.len() <= TAG_KEY_MAX_LEN
            && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        if !key_ok {
            return Err(AppError::InvalidArgs(format!(
                "tags: key {k:?} must be 1-{TAG_KEY_MAX_LEN} characters of letters, digits, '_', '.', '-'"
            )));
        }
        match v.as_str() {
            Some(s) if s.len() <= TAG_VALUE_MAX_LEN => out.insert(k.clone(), s.to_string()),
            _ => {
                return Err(AppError::InvalidArgs(format!(
                    "tags: value of {k:?} must be a string of at most {TAG_VALUE_MAX_LEN} bytes"
                )));
            }
        };
    }
    Ok(out)
}

/// `host.ts.put {series, value, ts?, tags?}` -> `{series, points, oldest_ts}`.
pub async fn put(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let series = series_arg(args)?;
    let value = args
        .get("value")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .ok_or_else(|| AppError::InvalidArgs("value: required, a finite number".into()))?;
    let ts = match args.get("ts").filter(|t| !t.is_null()) {
        None => now_unix(),
        Some(t) => t
            .as_i64()
            .filter(|t| *t >= 0)
            .ok_or_else(|| AppError::InvalidArgs("ts: must be unix seconds (a non-negative integer)".into()))?,
    };
    let tags = tags_arg(args)?;
    let tags_json = (!tags.is_empty()).then(|| serde_json::to_string(&tags).unwrap_or_default());

    let max = plan_of(state, &tenant.plan)?.ts_points_max;
    let plan_name = tenant.plan.clone();
    let catalog = state.plans.clone();
    let billing_mode = state.billing_config.billing_mode();
    let path = tenant_db_path(state, tenant.id);
    let series_for_conn = series.clone();

    let (points, oldest) = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        ensure_table(conn)?;
        let used: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {TS_TABLE} WHERE series = ?1"),
            [&series_for_conn],
            |r| r.get(0),
        )?;
        if used + 1 > max {
            return Err(over_quota(&catalog, billing_mode, &plan_name, max, used));
        }
        conn.execute(
            &format!("INSERT INTO {TS_TABLE} (series, ts, value, tags_json) VALUES (?1, ?2, ?3, ?4)"),
            rusqlite::params![series_for_conn, ts, value, tags_json],
        )?;
        let oldest: i64 = conn.query_row(
            &format!("SELECT MIN(ts) FROM {TS_TABLE} WHERE series = ?1"),
            [&series_for_conn],
            |r| r.get(0),
        )?;
        Ok((used + 1, oldest))
    })
    .await?;

    Ok(json!({"series": series, "points": points, "oldest_ts": oldest}))
}

/// What a caller can do about a full series.
pub const QUOTA_REMEDY: &str = "host.ts.trim or billing.checkout";

/// `quota_exceeded` naming `ts_points_max`. The error envelope overwrites
/// `data.remedy` with the registry's generic sentence for the code, so the
/// verb-specific remedy rides in the message and in `data.next.remedy`.
fn over_quota(
    catalog: &crate::plans::PlanCatalog,
    billing_mode: &str,
    plan: &str,
    max: i64,
    used: i64,
) -> AppError {
    match crate::billing::quota_exceeded_for(catalog, billing_mode, plan, "ts_points_max", max, used, None) {
        AppError::Structured { code, message, mut data } => {
            if let Some(next) = data.get_mut("next").and_then(Value::as_object_mut) {
                next.insert("remedy".into(), json!(QUOTA_REMEDY));
            }
            AppError::Structured { code, message: format!("{message}; remedy: {QUOTA_REMEDY}"), data }
        }
        other => other,
    }
}

/// One bucket's row: `[offset, count, min, max, avg, p50, p95]` read back from SQL.
fn bucket_sql(tag_count: usize) -> String {
    let mut preds = String::new();
    for i in 0..tag_count {
        let path = 5 + i * 2;
        preds.push_str(&format!(" AND json_extract(tags_json, ?{path}) = ?{}", path + 1));
    }
    format!(
        "WITH pts AS (
            SELECT (ts - ?1) / ?2 AS b, value,
                   ROW_NUMBER() OVER w AS rn,
                   COUNT(*) OVER w AS cnt
            FROM {TS_TABLE}
            WHERE series = ?3 AND ts >= ?1 AND ts <= ?4{preds}
            WINDOW w AS (PARTITION BY (ts - ?1) / ?2 ORDER BY value
                         ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING)
        )
        SELECT b, MAX(cnt), MIN(value), MAX(value), AVG(value),
               MAX(CASE WHEN rn = (cnt * 50 + 99) / 100 THEN value END),
               MAX(CASE WHEN rn = (cnt * 95 + 99) / 100 THEN value END)
        FROM pts GROUP BY b ORDER BY b"
    )
}

/// `host.ts.get {series, bucket, from?, to?, tags?}` ->
/// `{series, bucket_s, from, to, buckets: [{ts, count, min, max, avg, p50, p95}]}`.
/// Buckets are anchored at `from` (bucket `ts` is `from + i * bucket_s`); a
/// bucket with no points is omitted.
pub async fn get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let series = series_arg(args)?;
    let bucket = arg_str(args, "bucket")?;
    let Some(bucket_s) = bucket_seconds(&bucket) else {
        return Err(refusal(
            "bucket",
            bucket.clone(),
            format!("bucket {bucket:?} is not allowed; use one of {}", allowed_bucket_names().join(", ")),
        ));
    };
    let now = now_unix();
    let time_arg = |name: &str, default: &str| -> Result<i64, AppError> {
        let raw = args.get(name).filter(|v| !v.is_null()).cloned().unwrap_or_else(|| json!(default));
        parse_time(&raw, now).ok_or_else(|| {
            refusal(name, raw.to_string(), format!("{name} {raw} is not a time; use one of {}", TIME_FORMS.join(", ")))
        })
    };
    let from = time_arg("from", "-24h")?;
    let to = time_arg("to", "now")?;
    if from >= to {
        return Err(refusal("from", from.to_string(), format!("from ({from}) must be earlier than to ({to})")));
    }
    if (to - from) / bucket_s >= BUCKETS_MAX {
        return Err(refusal(
            "from",
            from.to_string(),
            format!("range holds more than {BUCKETS_MAX} buckets of {bucket}; narrow from/to or pick a wider bucket"),
        ));
    }
    let tags = tags_arg(args)?;

    let path = tenant_db_path(state, tenant.id);
    let rows = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        ensure_table(conn)?;
        let interrupt = conn.get_interrupt_handle();
        std::thread::spawn(move || {
            std::thread::sleep(QUERY_TIME_CAP);
            interrupt.interrupt();
        });
        let mut params: Vec<SqlValue> =
            vec![from.into(), bucket_s.into(), series.clone().into(), to.into()];
        for (k, v) in &tags {
            params.push(format!("$.\"{k}\"").into());
            params.push(v.clone().into());
        }
        let mut stmt = conn.prepare(&bucket_sql(tags.len()))?;
        let mapped = stmt.query_map(params_from_iter(params), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, f64>(5)?,
                r.get::<_, f64>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in mapped {
            match row {
                Ok(r) => out.push(r),
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::OperationInterrupted =>
                {
                    return Err(AppError::Structured {
                        code: crate::tables::TABLE_BOUND_EXCEEDED,
                        message: format!("query exceeded bound 'time_cap_s' (limit {})", QUERY_TIME_CAP.as_secs()),
                        data: json!({"bound": "time_cap_s", "limit": QUERY_TIME_CAP.as_secs()}),
                    });
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(out)
    })
    .await?;

    let series_name = args.get("series").cloned().unwrap_or(Value::Null);
    let buckets: Vec<Value> = rows
        .into_iter()
        .map(|(b, count, min, max, avg, p50, p95)| {
            let mut o = Map::new();
            o.insert("ts".into(), json!(from + b * bucket_s));
            o.insert("count".into(), json!(count));
            o.insert("min".into(), json!(min));
            o.insert("max".into(), json!(max));
            o.insert("avg".into(), json!(avg));
            o.insert("p50".into(), json!(p50));
            o.insert("p95".into(), json!(p95));
            Value::Object(o)
        })
        .collect();
    Ok(json!({
        "series": series_name,
        "bucket_s": bucket_s,
        "from": from,
        "to": to,
        "buckets": buckets,
    }))
}

/// `host.ts.trim {series, keep_last? | before_ts?}` -> `{series, deleted, points, oldest_ts}`.
/// Exactly one of the two cuts: `keep_last` keeps the newest n points (by `ts`),
/// `before_ts` drops every point older than that unix second.
pub async fn trim(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let series = series_arg(args)?;
    let int_arg = |name: &str| -> Result<Option<i64>, AppError> {
        match args.get(name).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => v
                .as_i64()
                .filter(|n| *n >= 0)
                .map(Some)
                .ok_or_else(|| AppError::InvalidArgs(format!("{name}: must be a non-negative integer"))),
        }
    };
    let (keep_last, before_ts) = (int_arg("keep_last")?, int_arg("before_ts")?);
    if keep_last.is_some() == before_ts.is_some() {
        return Err(AppError::InvalidArgs("give exactly one of keep_last or before_ts".into()));
    }
    let path = tenant_db_path(state, tenant.id);
    let dry_run = crate::dryrun::active();
    let series_for_conn = series.clone();
    let (deleted, points, oldest) = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        ensure_table(conn)?;
        let deleted = match (keep_last, before_ts) {
            (Some(n), _) => conn.execute(
                &format!(
                    "DELETE FROM {TS_TABLE} WHERE series = ?1 AND rowid NOT IN \
                     (SELECT rowid FROM {TS_TABLE} WHERE series = ?1 ORDER BY ts DESC, rowid DESC LIMIT ?2)"
                ),
                rusqlite::params![series_for_conn, n],
            )?,
            (None, Some(t)) => conn.execute(
                &format!("DELETE FROM {TS_TABLE} WHERE series = ?1 AND ts < ?2"),
                rusqlite::params![series_for_conn, t],
            )?,
            (None, None) => 0,
        };
        let (points, oldest): (i64, Option<i64>) = conn.query_row(
            &format!("SELECT COUNT(*), MIN(ts) FROM {TS_TABLE} WHERE series = ?1"),
            [&series_for_conn],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((deleted as i64, points, oldest))
    })
    .await?;
    if dry_run {
        crate::dryrun::note_would("rows", json!({"table": TS_TABLE, "series": series, "delete": deleted}));
    }
    Ok(json!({"series": series, "deleted": deleted, "points": points, "oldest_ts": oldest}))
}

/// `host.ts.list {}` -> `{series: [{series, points, oldest_ts, newest_ts}], truncated}`, by
/// name, at most [`SERIES_LIST_MAX`] series.
pub async fn list(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let path = tenant_db_path(state, tenant.id);
    let mut rows = with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        ensure_table(conn)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT series, COUNT(*), MIN(ts), MAX(ts) FROM {TS_TABLE} GROUP BY series ORDER BY series LIMIT ?1"
        ))?;
        let rows = stmt
            .query_map([SERIES_LIST_MAX + 1], |r| {
                Ok(json!({
                    "series": r.get::<_, String>(0)?,
                    "points": r.get::<_, i64>(1)?,
                    "oldest_ts": r.get::<_, i64>(2)?,
                    "newest_ts": r.get::<_, i64>(3)?,
                }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await?;
    let truncated = rows.len() as i64 > SERIES_LIST_MAX;
    rows.truncate(SERIES_LIST_MAX as usize);
    Ok(json!({"series": rows, "truncated": truncated}))
}
