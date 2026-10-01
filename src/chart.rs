//! PRD-mcphost-chart-in-a-minute: `host.table.chart(sql, title?, mark?,
//! share?)` and `host.table.charts()` -- one SQL call returns a
//! `result-profile.v1` profile, a chart recommendation, an inline-data
//! Vega-Lite v5 spec, and a caption whose numbers are recomputed from the
//! rows, not guessed by a model. `share: true` stores the spec and caption
//! in the tenant's own `_mcphost_charts` table and returns a signed,
//! unauthenticated `/charts/{id}` link (P1 requirement 6).
//!
//! The chain (`mqo-chart-recommender::recommend`, `mqo-vega-emitter::emit`,
//! `mqo-chart-caption::generate_caption`) is vendored unchanged from
//! ai-stack (`vendor/VENDOR.md`, AC1); this module is the one piece mcphost
//! writes itself -- a `result-profile.v1` profiler over `host.table.query`
//! rows (requirement 2), since ai-stack's own profiler
//! (`mqo-result-profiler`, vendored for its `ResultProfile` types only)
//! reads an MQO response mcphost has none of.
//!
//! Role classification (requirement 2's "measure for numeric columns that
//! are not keys or ids, else dimension") reuses `tables_model.rs`'s own
//! [`crate::tables_model::ColBuilder`] scoring -- the same
//! key/category/measure/date/id thresholds `host.table.describe` uses --
//! rather than a second set of thresholds. A result column is looked up
//! against the query's own `FROM` table's declared schema first: a
//! genuinely declared column (e.g. `category`) is scored by
//! `ColBuilder::finish`; a computed/aggregate expression with no matching
//! declared column (e.g. `SUM(amount) AS total`, or the un-aliased
//! `SUM(amount)`/`COUNT(*)` SQLite itself names) is always a measure when
//! its values are numeric -- this sidesteps `ColBuilder`'s own `key` rule
//! occasionally firing on a small aggregated result (every `SUM` value
//! happening to be distinct) the way it would for a real multi-thousand-row
//! table sample.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use mqo_chart_caption::{Caption, CaptionConfig, CaptionInput};
use mqo_chart_recommender::Recommendation;
use mqo_chart_vocab::{DataType, Mark, Role};
use mqo_result_profiler::{ColumnProfile, MeasureRange, ResultProfile};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::db::Tenant;
use crate::enduser::EndUser;
use crate::errors::AppError;
use crate::state::AppState;
use crate::tables::{self, ColumnType};
use crate::tables_model::{self, ColBuilder};

const CHARTS_TABLE: &str = "_mcphost_charts";
/// P1 requirement 6/AC10: a tenant holds at most this many stored charts;
/// storing past it evicts the oldest.
const CHARTS_MAX: i64 = 100;

// ---- profiling -----------------------------------------------------------

/// Best-effort: the single table named in `sql`'s `FROM` clause, when `sql`
/// is a plain `SELECT ... FROM <table>` with no join. `None` for anything
/// else (a join, a CTE, a subquery) -- every result column then falls back
/// to the computed-expression path below, which is still correct, just less
/// precise about which numeric columns are really dimensions.
fn base_table_name(sql: &str) -> Option<String> {
    let statements =
        sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::GenericDialect {}, sql).ok()?;
    let [sqlparser::ast::Statement::Query(query)] = statements.as_slice() else {
        return None;
    };
    let sqlparser::ast::SetExpr::Select(select) = query.body.as_ref() else {
        return None;
    };
    let [table_with_joins] = select.from.as_slice() else {
        return None;
    };
    if !table_with_joins.joins.is_empty() {
        return None;
    }
    match &table_with_joins.relation {
        sqlparser::ast::TableFactor::Table { name, .. } => Some(name.to_string()),
        _ => None,
    }
}

/// Best-effort SQLite runtime type of a computed/aggregate column, from its
/// own returned values -- used only when the column doesn't match a
/// declared column (so there is no [`ColumnType`] to look up).
fn guess_column_type(values: &[Value]) -> ColumnType {
    for v in values {
        if v.is_i64() || v.is_u64() {
            return ColumnType::Integer;
        }
        if v.is_f64() {
            return ColumnType::Real;
        }
        if v.is_string() {
            return ColumnType::Text;
        }
    }
    ColumnType::Text
}

/// Technical considerations: "`is_temporal` is true for columns the
/// table-model date classifier accepts ... and for SQLite
/// `date()`/`strftime` outputs, which arrive as text" -- both are plain
/// ISO-ish strings from this function's point of view, so one check (reused
/// from `tables_model`, not a second threshold) covers both.
fn column_is_temporal(values: &[Value]) -> bool {
    let mut non_null = 0i64;
    let mut parseable = 0i64;
    for v in values {
        let Value::String(s) = v else { continue };
        non_null += 1;
        if tables_model::is_iso_date(s) {
            parseable += 1;
        }
    }
    non_null > 0 && (parseable as f64 / non_null as f64) >= tables_model::DATE_MIN_PARSE_RATE
}

/// A human label from an identifier-shaped column name (`"total_amount"` ->
/// `"Total amount"`); a computed expression's own SQLite-assigned name
/// (e.g. `"SUM(amount)"`) is left as-is rather than mangled.
fn humanize_label(name: &str) -> String {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return name.to_string();
    }
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => spaced,
    }
}

fn declared_schema_sync(conn: &Connection, table: &str) -> BTreeMap<String, ColumnType> {
    tables::load_schema_sync(conn, table).map(|s| s.columns).unwrap_or_default()
}

/// requirement 2: builds a `result-profile.v1` value from `sql`'s own
/// column names (read off the first row -- an empty result has none, so it
/// short-circuits to an all-zero profile the recommender reads as "zero
/// rows", requirement 4/AC5) and `rows`.
async fn build_profile(
    state: &AppState,
    tenant: &Tenant,
    sql: &str,
    rows: &[Value],
) -> Result<ResultProfile, AppError> {
    if rows.is_empty() {
        return Ok(ResultProfile::new(0, Vec::new()));
    }

    let column_names: Vec<String> = match &rows[0] {
        Value::Object(map) => map.keys().cloned().collect(),
        _ => Vec::new(),
    };

    let declared = match base_table_name(sql) {
        Some(table) => {
            let path = tables::tenant_db_path(state, tenant.id);
            tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
                Ok(declared_schema_sync(conn, &table))
            })
            .await
            .unwrap_or_default()
        }
        None => BTreeMap::new(),
    };

    let mut columns = Vec::with_capacity(column_names.len());
    for name in &column_names {
        let values: Vec<Value> = rows.iter().map(|r| r.get(name).cloned().unwrap_or(Value::Null)).collect();
        let is_declared = declared.contains_key(name);
        let ty = declared.get(name).copied().unwrap_or_else(|| guess_column_type(&values));

        let mut builder = ColBuilder::default();
        for v in &values {
            builder.push(v.clone());
        }
        let analysis = builder.finish(ty, values.len() as i64);

        let is_numeric = matches!(ty, ColumnType::Integer | ColumnType::Real);
        // See this module's own doc comment: an undeclared (computed)
        // numeric column is always a measure; a declared one defers to
        // tables_model's own role scoring.
        let is_measure = is_numeric && (!is_declared || analysis.role == "measure");
        let is_temporal = column_is_temporal(&values);

        let data_type = if is_measure {
            DataType::Quantitative
        } else if is_temporal {
            DataType::Temporal
        } else {
            DataType::Nominal
        };

        let measure_range = if is_measure {
            match (analysis.min.as_f64(), analysis.max.as_f64()) {
                (Some(min), Some(max)) => Some(MeasureRange { min, max }),
                _ => None,
            }
        } else {
            None
        };

        columns.push(ColumnProfile {
            name: name.clone(),
            label: humanize_label(name),
            role: if is_measure { Role::Measure } else { Role::Dimension },
            data_type,
            cardinality: analysis.distinct,
            null_rate: analysis.null_share,
            measure_range,
            is_temporal,
        });
    }

    Ok(ResultProfile::new(rows.len() as i64, columns))
}

// ---- mark override --------------------------------------------------------

fn invalid_mark_error(requested: &str, rec: &Recommendation) -> AppError {
    let mut allowed: Vec<&str> = vec![rec.mark.as_str()];
    allowed.extend(rec.alternatives.iter().map(|m| m.as_str()));
    AppError::Structured {
        code: "chart_invalid_mark",
        message: format!(
            "mark '{requested}' is not allowed for this result; allowed marks: {}",
            allowed.join(", ")
        ),
        data: json!({"requested": requested, "allowed": allowed}),
    }
}

/// requirement 3/AC7: `mark` must be the recommended mark or one of its
/// `alternatives` -- anything else (including a mark this vocabulary
/// doesn't even have, like `"pie"`) is a validation error naming the
/// allowed set, and (since this runs before any storage) nothing is stored.
fn apply_mark_override(rec: Recommendation, requested: &str) -> Result<Recommendation, AppError> {
    let Some(mark) = Mark::parse(requested) else {
        return Err(invalid_mark_error(requested, &rec));
    };
    let allowed = std::iter::once(rec.mark).chain(rec.alternatives.iter().copied());
    if !allowed.clone().any(|m| m == mark) {
        return Err(invalid_mark_error(requested, &rec));
    }
    Ok(Recommendation { mark, ..rec })
}

// ---- storage (share: true) -------------------------------------------------

fn ensure_charts_table_sync(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {CHARTS_TABLE} (
            id TEXT PRIMARY KEY,
            title TEXT,
            vega_lite_json TEXT NOT NULL,
            caption_json TEXT NOT NULL,
            created_unix INTEGER NOT NULL
        );"
    ))?;
    Ok(())
}

/// Stores one chart in the tenant's own table file; when this pushes the
/// tenant past [`CHARTS_MAX`], evicts (and returns) the oldest chart's id
/// (insertion order via `rowid`, since `id` is a `TEXT PRIMARY KEY` and so
/// does not alias it).
#[allow(clippy::too_many_arguments)] // same call export::build_tar_gz already makes -- every argument is a distinct piece of the one row being written, not a bundle begging for a struct
async fn store_chart(
    state: &AppState,
    tenant: &Tenant,
    id: &str,
    title: Option<&str>,
    vega_lite_json: String,
    caption_json: String,
    created_unix: i64,
) -> Result<Option<String>, AppError> {
    let path = tables::tenant_db_path(state, tenant.id);
    let id_owned = id.to_string();
    let title_owned = title.map(str::to_string);
    tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        ensure_charts_table_sync(conn)?;
        conn.execute(
            &format!(
                "INSERT INTO {CHARTS_TABLE} (id, title, vega_lite_json, caption_json, created_unix) \
                 VALUES (?1, ?2, ?3, ?4, ?5)"
            ),
            params![id_owned, title_owned, vega_lite_json, caption_json, created_unix],
        )?;
        let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {CHARTS_TABLE}"), [], |r| r.get(0))?;
        if count <= CHARTS_MAX {
            return Ok(None);
        }
        let oldest: String = conn.query_row(
            &format!("SELECT id FROM {CHARTS_TABLE} ORDER BY rowid ASC LIMIT 1"),
            [],
            |r| r.get(0),
        )?;
        conn.execute(&format!("DELETE FROM {CHARTS_TABLE} WHERE id = ?1"), params![oldest])?;
        Ok(Some(oldest))
    })
    .await
}

/// P1 requirement 8: this tenant's stored charts, newest first.
pub async fn table_charts_list(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let path = tables::tenant_db_path(state, tenant.id);
    let rows = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        ensure_charts_table_sync(conn)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT id, title, created_unix FROM {CHARTS_TABLE} ORDER BY created_unix DESC, rowid DESC"
        ))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .await?;

    let charts: Vec<Value> = rows
        .into_iter()
        .map(|(id, title, created_unix)| {
            json!({
                "id": id,
                "title": title,
                "created_unix": created_unix,
                "expires_unix": created_unix + crate::export::EXPORT_URL_TTL_SECS,
            })
        })
        .collect();
    Ok(json!({"charts": charts}))
}

// ---- signed share URL -----------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn chart_path(id: &str) -> String {
    format!("/charts/{id}")
}

/// Same HMAC-over-`"{path}:{expires}"` scheme `export::sign` uses, keyed by
/// the sharing tenant's own `key_hash` (technical considerations: "the same
/// HMAC scheme and 24-hour expiry `src/export.rs` uses").
fn sign(key_hash: &str, id: &str, expires_unix: i64) -> String {
    let message = format!("{}:{expires_unix}", chart_path(id));
    hex_encode(&crate::billing::hmac_sha256(key_hash.as_bytes(), message.as_bytes()))
}

/// `pub` (like `export::signed_download_url`): both the real caller below
/// and this crate's own test suite (an already-expired or
/// altered-signature URl AC9 needs) mint one.
pub fn signed_chart_url(public_url: &str, key_hash: &str, id: &str, expires_unix: i64) -> String {
    let sig = sign(key_hash, id, expires_unix);
    format!("{public_url}{}?exp={expires_unix}&sig={sig}", chart_path(id))
}

// ---- host.table.chart / host.table.charts ---------------------------------

/// requirement 3: `host.table.chart(sql, title?, mark?, share?)` --
/// `sql` runs through [`tables::table_query`] (requirement/technical
/// considerations: the same read-only path, `ROW_CAP`, `QUERY_TIME_CAP`
/// `host.table.query` uses), then the vendored chain turns the result into
/// a recommendation, a Vega-Lite spec, and a caption.
pub async fn table_chart(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    let sql = tables::arg_str(args, "sql")?;
    let title = tables::arg_str_opt(args, "title");
    let requested_mark = tables::arg_str_opt(args, "mark");
    let share = args.get("share").and_then(Value::as_bool).unwrap_or(false);

    let query_result = tables::table_query(state, tenant, &json!({"sql": sql}), end_user).await?;
    let rows: Vec<Value> = query_result["rows"].as_array().cloned().unwrap_or_default();

    let profile = build_profile(state, tenant, &sql, &rows).await?;
    let profile_json = serde_json::to_value(&profile)
        .map_err(|e| AppError::Internal(format!("profile serialize: {e}")))?;

    let mut recommendation = mqo_chart_recommender::recommend(&profile_json);
    if let Some(requested) = &requested_mark {
        recommendation = apply_mark_override(recommendation, requested)?;
    }

    let rows_value = Value::Array(rows.clone());
    let vega_lite = mqo_vega_emitter::emit(&recommendation, &rows_value);
    let caption: Caption = mqo_chart_caption::generate_caption(
        &CaptionInput { profile: profile.clone(), rows: rows.clone() },
        &CaptionConfig::default(),
    );

    let mut out = json!({
        "schema": "chart.v1",
        "title": title,
        "row_count": rows.len() as i64,
        "profile": profile_json,
        "recommendation": {
            "mark": recommendation.mark,
            "encoding": recommendation.encoding,
            "rationale": recommendation.rationale,
            "alternatives": recommendation.alternatives,
        },
        "vega_lite": vega_lite,
        "caption": {"headline": caption.headline, "facts": caption.facts},
    });

    if share {
        let id = crate::state::new_ulid();
        let created_unix = crate::state::now_unix();
        let vega_lite_json = serde_json::to_string(&vega_lite)
            .map_err(|e| AppError::Internal(format!("vega_lite serialize: {e}")))?;
        let caption_json =
            serde_json::to_string(&json!({"headline": caption.headline, "facts": caption.facts}))
                .map_err(|e| AppError::Internal(format!("caption serialize: {e}")))?;
        let evicted =
            store_chart(state, tenant, &id, title.as_deref(), vega_lite_json, caption_json, created_unix).await?;
        state.db.record_chart_owner(tenant.id, id.clone(), created_unix).await?;
        if let Some(evicted_id) = evicted {
            state.db.delete_chart_owner(evicted_id).await?;
        }
        let expires_unix = created_unix + crate::export::EXPORT_URL_TTL_SECS;
        let share_url = signed_chart_url(&state.public_url, &tenant.key_hash, &id, expires_unix);
        if let Some(obj) = out.as_object_mut() {
            obj.insert("chart_id".to_string(), json!(id));
            obj.insert("share_url".to_string(), json!(share_url));
        }
    }

    Ok(out)
}

// ---- GET /charts/{id} ------------------------------------------------------

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// P1 requirement 6/AC8: the spec inline, `vega-embed` from jsdelivr, the
/// caption's headline and facts -- no tenant identifiers anywhere in the
/// markup (nothing here ever reads `tenant.namespace`/`tenant.id`/
/// `tenant.key_hash`).
fn render_chart_html(title: Option<&str>, vega_lite_json: &str, caption_json: &str) -> String {
    let caption: Value = serde_json::from_str(caption_json).unwrap_or_else(|_| json!({"headline": "", "facts": []}));
    let headline = caption["headline"].as_str().unwrap_or("");
    let facts_html: String = caption["facts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|f| {
            let label = f["label"].as_str().unwrap_or("");
            let value = f["value"].to_string();
            format!("<li>{}: {}</li>", html_escape(label), html_escape(&value))
        })
        .collect();
    let page_title = title.unwrap_or("Chart");

    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>{title}</title>
<script src="https://cdn.jsdelivr.net/npm/vega@5"></script>
<script src="https://cdn.jsdelivr.net/npm/vega-lite@5"></script>
<script src="https://cdn.jsdelivr.net/npm/vega-embed@6"></script>
</head>
<body>
<h1>{title}</h1>
<div id="chart"></div>
<p>{headline}</p>
<ul>{facts}</ul>
<script type="application/json" id="mcphost-chart-spec">{spec}</script>
<script>
vegaEmbed('#chart', JSON.parse(document.getElementById('mcphost-chart-spec').textContent));
</script>
</body>
</html>
"#,
        title = html_escape(page_title),
        headline = html_escape(headline),
        facts = facts_html,
        spec = vega_lite_json,
    )
}

/// `GET /charts/{id}?exp=<unix>&sig=<hex>` -- unauthenticated, same
/// signature-is-the-whole-auth-story shape as `export::download`. The chart
/// id has no tenant in its own URL, so ownership is resolved through the
/// global `chart_index` table ([`crate::db::Db::find_chart_owner`]) before
/// the owning tenant's own table file is opened.
pub async fn download(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Ok(Some(tenant_id)) = state.db.find_chart_owner(id.clone()).await else {
        return (StatusCode::NOT_FOUND, "chart not found").into_response();
    };
    let Ok(Some(tenant)) = state.db.find_tenant_by_id(tenant_id).await else {
        return (StatusCode::NOT_FOUND, "chart not found").into_response();
    };
    let (Some(expires_raw), Some(sig)) = (params.get("exp"), params.get("sig")) else {
        return (StatusCode::FORBIDDEN, "missing signature").into_response();
    };
    let Ok(expires_unix) = expires_raw.parse::<i64>() else {
        return (StatusCode::FORBIDDEN, "invalid signature").into_response();
    };
    let expected = sign(&tenant.key_hash, &id, expires_unix);
    if !constant_time_eq(sig.as_bytes(), expected.as_bytes()) {
        return (StatusCode::FORBIDDEN, "invalid signature").into_response();
    }
    // AC9: past its expiry, a validly-signed URL reads 410 -- the
    // signature was genuine, only stale.
    if crate::state::now_unix() > expires_unix {
        return (StatusCode::GONE, "chart link expired").into_response();
    }

    let path = tables::tenant_db_path(&state, tenant.id);
    let id_for_conn = id.clone();
    let row = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        ensure_charts_table_sync(conn)?;
        conn.query_row(
            &format!("SELECT title, vega_lite_json, caption_json FROM {CHARTS_TABLE} WHERE id = ?1"),
            params![id_for_conn],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(AppError::from)
    })
    .await;

    let Ok(Some((title, vega_lite_json, caption_json))) = row else {
        return (StatusCode::NOT_FOUND, "chart not found").into_response();
    };
    let html = render_chart_html(title.as_deref(), &vega_lite_json, &caption_json);
    (StatusCode::OK, [("content-type", "text/html; charset=utf-8")], html).into_response()
}
