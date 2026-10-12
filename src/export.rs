//! PRD-mcphost-tenant-data-export: `host.export` -- a tenant can take its
//! tool sources, state, secrets' names (never values), runs and threads
//! with it. Builds a `.tar.gz` archive as a background job (reusing the
//! `runs` ledger `runs.rs` already owns), stores it on disk under this
//! host's data dir, and hands back a signed download URL valid 24 hours
//! (technical considerations: "HMAC over path+expiry with the existing
//! session-key material; no new secret" -- the tenant's own `key_hash`,
//! already stored, is that material).
//!
//! Unlike an ordinary `host.tool_call(..., async: true)` job, an export has
//! no `tools` row or `Kind` to dispatch through, so it does not go through
//! `runs::enqueue`/the executor's leasing loop -- see
//! [`crate::db::Db::start_export_run`]'s doc comment for why the row is
//! inserted straight into `running` instead of `queued`. `handler.rs` is
//! the only place that dispatches `"host.export"` here from a wire call;
//! `http.rs` is the only place that routes `GET /exports/{run_id}` to
//! [`download`].

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use tar::{Builder, Header};

use crate::db::{MessageRow, RunRow, StartExportRun, Tenant, ToolRow};
use crate::errors::AppError;
use crate::state::AppState;

pub const EXPORT_TOOL_NAME: &str = "host.export";
/// PRD-mcphost-end-user-audit-and-revoke requirement 6 (AC8): a distinct
/// `runs.tool_name` from [`EXPORT_TOOL_NAME`] so a per-subject export's
/// own "one running at a time" check ([`crate::db::Db::start_export_run`])
/// never collides with (or rejoins) a whole-tenant export.
pub const ENDUSER_EXPORT_TOOL_NAME: &str = "host.enduser.export";
/// PRD-mcphost-result-handles P1 requirement 7 (AC8): a third distinct
/// `runs.tool_name`, same "own dedupe key, own runs row" reasoning
/// [`ENDUSER_EXPORT_TOOL_NAME`]'s doc already gives -- a handle export
/// running for this tenant never collides with (or rejoins) a whole-tenant
/// or end-user one, and [`download`] tells a CSV file apart from a
/// `.tar.gz` one by which of the three this run's own `tool_name` is.
pub const HANDLE_EXPORT_TOOL_NAME: &str = "host.table.handle_export";
pub(crate) const EXPORT_DIR: &str = "exports";
/// AC2: a signed download URL is valid for this long.
pub const EXPORT_URL_TTL_SECS: i64 = crate::signed_url::DEFAULT_TTL_SECS;

fn export_path(run_id: &str) -> String {
    format!("/exports/{run_id}")
}

/// The full signed URL a completed export's run result carries, minted by
/// [`crate::signed_url`] like every other signed route. `pub` so both
/// [`build_archive`] (the real caller) and this crate's own test suite
/// (AC2's "after 24h, 410" case, which needs a validly-signed but
/// already-expired URL no real 24-hour wait could produce) can mint one.
pub fn signed_download_url(public_url: &str, key_hash: &str, run_id: &str, expires_unix: i64) -> String {
    crate::signed_url::mint(public_url, key_hash, &export_path(run_id), "expires", expires_unix)
}

fn arg_tools_filter(args: &Value) -> Result<Option<Vec<String>>, AppError> {
    match args.get("tools") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                let name = item.as_str().ok_or_else(|| {
                    AppError::InvalidArgs("tools: every entry must be a string".to_string())
                })?;
                names.push(name.to_string());
            }
            Ok(Some(names))
        }
        Some(_) => Err(AppError::InvalidArgs(
            "tools: must be an array of strings".to_string(),
        )),
    }
}

/// `host.export {tools?: [name, ...]}` (P0 requirements 1-3): starts (or,
/// per requirement 3 / AC3, rejoins) a background job that builds this
/// tenant's whole data archive. Returns immediately -- the archive itself
/// is built by [`run_export_job`], spawned below, and finalized into the
/// same `runs` ledger every other async job uses.
pub async fn export(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let tools_filter = arg_tools_filter(args)?;
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let run_id = crate::state::new_ulid();
    let args_json = serde_json::to_string(args)
        .map_err(|e| AppError::Internal(format!("args serialize: {e}")))?;
    let outcome = state
        .db
        .start_export_run(
            tenant.id,
            run_id.clone(),
            EXPORT_TOOL_NAME.to_string(),
            plan.job_max_s,
            args_json,
        )
        .await?;
    // AC3: an export already `running` for this tenant -- the caller gets
    // that run's id back, not a second job.
    let run_id = match outcome {
        StartExportRun::AlreadyRunning(existing) => {
            return Ok(json!({"run_id": existing, "status": "running"}));
        }
        StartExportRun::Started(new_id) => new_id,
    };
    let spawn_state = state.clone();
    let spawn_tenant = tenant.clone();
    let spawn_run_id = run_id.clone();
    tokio::spawn(async move {
        run_export_job(spawn_state, spawn_tenant, spawn_run_id, tools_filter).await;
    });
    Ok(json!({"run_id": run_id, "status": "running"}))
}

/// Runs one export to completion and finalizes its `runs` row -- same
/// "never panic out of the spawn, degrade to an error instead" shape as
/// `runs::run_one_job`.
async fn run_export_job(
    state: AppState,
    tenant: Tenant,
    run_id: String,
    tools_filter: Option<Vec<String>>,
) {
    let start = std::time::Instant::now();
    let outcome = build_archive(&state, &tenant, &run_id, tools_filter.as_deref()).await;
    finish_run(&state, &tenant, run_id, start, outcome).await;
}

/// Stores a finished background job's result as the single-part
/// `runs/<run_id>/part/0` state value `host.runs.get`/`host.runs.part` read
/// (PRD-mcphost-run-result-overflow-to-state: an export's or import's result
/// is a small object, always the one-part case) and finalizes its `runs` row.
pub(crate) async fn finish_run(
    state: &AppState,
    tenant: &Tenant,
    run_id: String,
    start: std::time::Instant,
    outcome: Result<Value, AppError>,
) {
    let finished_unix = crate::state::now_unix();
    let duration_ms = start.elapsed().as_millis() as i64;

    let (status, result_ref, error_class) = match outcome {
        Ok(result_value) => {
            let value_json = serde_json::to_string(&result_value).unwrap_or_else(|_| "null".to_string());
            let bytes = value_json.len() as i64;
            let set_args = json!({"key": crate::runs::part_key(&run_id, 0), "value": result_value});
            match crate::tenant_state::state_set(state, tenant, &set_args, None).await {
                Ok(_) => {
                    let result_ref = json!({
                        "parts": 1,
                        "bytes": bytes,
                        "content_type": "application/json",
                    })
                    .to_string();
                    ("done".to_string(), Some(result_ref), None)
                }
                Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
            }
        }
        Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
    };

    let _ = state
        .db
        .finalize_run(run_id, tenant.id, status, result_ref, error_class, None, finished_unix, duration_ms)
        .await;
}

/// `host.enduser.export {subject}` (requirement 6 / AC8): same
/// runs-ledger/background-job/signed-download shape as [`export`] above,
/// scoped to one end user's own state rows and call history rather than
/// the whole tenant -- "reuses export.rs's bundle writer with a filter"
/// (technical considerations), the filter here being `end_user_subject`
/// rather than a `tools` list.
pub async fn enduser_export(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let subject = args
        .get("subject")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'subject'".to_string()))?;
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let run_id = crate::state::new_ulid();
    let args_json = serde_json::to_string(args)
        .map_err(|e| AppError::Internal(format!("args serialize: {e}")))?;
    let outcome = state
        .db
        .start_export_run(
            tenant.id,
            run_id.clone(),
            ENDUSER_EXPORT_TOOL_NAME.to_string(),
            plan.job_max_s,
            args_json,
        )
        .await?;
    let run_id = match outcome {
        StartExportRun::AlreadyRunning(existing) => {
            return Ok(json!({"run_id": existing, "status": "running"}));
        }
        StartExportRun::Started(new_id) => new_id,
    };
    let spawn_state = state.clone();
    let spawn_tenant = tenant.clone();
    let spawn_run_id = run_id.clone();
    tokio::spawn(async move {
        run_enduser_export_job(spawn_state, spawn_tenant, spawn_run_id, subject).await;
    });
    Ok(json!({"run_id": run_id, "status": "running"}))
}

async fn run_enduser_export_job(state: AppState, tenant: Tenant, run_id: String, subject: String) {
    let start = std::time::Instant::now();
    let outcome = build_enduser_archive(&state, &tenant, &run_id, &subject).await;
    let finished_unix = crate::state::now_unix();
    let duration_ms = start.elapsed().as_millis() as i64;

    let (status, result_ref, error_class) = match outcome {
        Ok(result_value) => {
            // PRD-mcphost-run-result-overflow-to-state: the same single-part
            // `runs/<run_id>/part/0` convention `run_export_job` above uses
            // -- an end-user export's result (a small manifest/
            // download_url object) never overflows `MAX_TOOL_OUTPUT_BYTES`,
            // so this is always the one-part case, and `host.runs.get`
            // reads it through the exact same path as any other job's.
            let value_json = serde_json::to_string(&result_value).unwrap_or_else(|_| "null".to_string());
            let bytes = value_json.len() as i64;
            let set_args = json!({"key": crate::runs::part_key(&run_id, 0), "value": result_value});
            match crate::tenant_state::state_set(&state, &tenant, &set_args, None).await {
                Ok(_) => {
                    let result_ref = json!({
                        "parts": 1,
                        "bytes": bytes,
                        "content_type": "application/json",
                    })
                    .to_string();
                    ("done".to_string(), Some(result_ref), None)
                }
                Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
            }
        }
        Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
    };

    let _ = state
        .db
        .finalize_run(run_id, tenant.id, status, result_ref, error_class, None, finished_unix, duration_ms)
        .await;
}

/// `host.table.handle_export {handle}` (P1 requirement 7/AC8): same
/// runs-ledger/background-job/signed-download shape as [`export`]/
/// [`enduser_export`] above, writing the handle's own rows as CSV
/// (`tables::handle_export_rows` -- unbounded by `ROW_CAP`) instead of a
/// `.tar.gz` bundle.
pub async fn handle_export(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let handle = args
        .get("handle")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'handle'".to_string()))?;
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    let run_id = crate::state::new_ulid();
    let args_json = serde_json::to_string(args)
        .map_err(|e| AppError::Internal(format!("args serialize: {e}")))?;
    let outcome = state
        .db
        .start_export_run(
            tenant.id,
            run_id.clone(),
            HANDLE_EXPORT_TOOL_NAME.to_string(),
            plan.job_max_s,
            args_json,
        )
        .await?;
    let run_id = match outcome {
        StartExportRun::AlreadyRunning(existing) => {
            return Ok(json!({"run_id": existing, "status": "running"}));
        }
        StartExportRun::Started(new_id) => new_id,
    };
    let spawn_state = state.clone();
    let spawn_tenant = tenant.clone();
    let spawn_run_id = run_id.clone();
    tokio::spawn(async move {
        run_handle_export_job(spawn_state, spawn_tenant, spawn_run_id, handle).await;
    });
    Ok(json!({"run_id": run_id, "status": "running"}))
}

async fn run_handle_export_job(state: AppState, tenant: Tenant, run_id: String, handle: String) {
    let start = std::time::Instant::now();
    let outcome = build_handle_csv(&state, &tenant, &run_id, &handle).await;
    let finished_unix = crate::state::now_unix();
    let duration_ms = start.elapsed().as_millis() as i64;

    let (status, result_ref, error_class) = match outcome {
        Ok(result_value) => {
            // PRD-mcphost-run-result-overflow-to-state: same single-part
            // convention `run_export_job`/`run_enduser_export_job` use
            // above -- a handle export's own result (download_url, size,
            // row_count) never overflows `MAX_TOOL_OUTPUT_BYTES`.
            let value_json = serde_json::to_string(&result_value).unwrap_or_else(|_| "null".to_string());
            let bytes = value_json.len() as i64;
            let set_args = json!({"key": crate::runs::part_key(&run_id, 0), "value": result_value});
            match crate::tenant_state::state_set(&state, &tenant, &set_args, None).await {
                Ok(_) => {
                    let result_ref = json!({
                        "parts": 1,
                        "bytes": bytes,
                        "content_type": "application/json",
                    })
                    .to_string();
                    ("done".to_string(), Some(result_ref), None)
                }
                Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
            }
        }
        Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
    };

    let _ = state
        .db
        .finalize_run(run_id, tenant.id, status, result_ref, error_class, None, finished_unix, duration_ms)
        .await;
}

/// RFC 4180-shaped CSV field escaping: a field containing a comma, quote,
/// or newline is wrapped in `"..."` with embedded `"` doubled; everything
/// else passes through -- same hand-rolled-encoding stance this crate
/// already takes for ULIDs/RFC 3339/HMAC-SHA256 rather than adding a `csv`
/// dependency for one small, fully-specified format.
fn csv_field(value: &Value) -> String {
    let raw = match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if raw.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", raw.replace('"', "\"\""))
    } else {
        raw
    }
}

/// AC8: `row_count` data rows plus a header -- one CSV line per handle
/// row, in the same column order `tables::handle_export_rows` returns
/// (the handle's own declaration order), never truncated by `ROW_CAP`
/// (that bound exists for `host.table.query`'s own result size, not for
/// exporting a handle that already exists specifically to hold more rows
/// than that).
async fn build_handle_csv(
    state: &AppState,
    tenant: &Tenant,
    run_id: &str,
    handle: &str,
) -> Result<Value, AppError> {
    let (columns, rows) = crate::tables::handle_export_rows(state, tenant, handle).await?;

    let mut csv = columns.iter().map(|c| csv_field(&json!(c))).collect::<Vec<_>>().join(",");
    csv.push_str("\r\n");
    for row in &rows {
        let line = columns.iter().map(|c| csv_field(&row[c])).collect::<Vec<_>>().join(",");
        csv.push_str(&line);
        csv.push_str("\r\n");
    }
    let csv_bytes = csv.into_bytes();

    let exports_dir = state.db.data_dir().join(EXPORT_DIR);
    tokio::fs::create_dir_all(&exports_dir)
        .await
        .map_err(|e| AppError::Storage(format!("create exports dir: {e}")))?;
    let csv_path = exports_dir.join(format!("{run_id}.csv"));
    tokio::fs::write(&csv_path, &csv_bytes)
        .await
        .map_err(|e| AppError::Storage(format!("write handle export csv: {e}")))?;

    // P1 requirement 7: valid 24h, same signed-URL scheme as the other
    // export kinds.
    let expires_unix = crate::state::now_unix() + EXPORT_URL_TTL_SECS;
    let download_url = signed_download_url(&state.public_url, &tenant.key_hash, run_id, expires_unix);

    Ok(json!({
        "download_url": download_url,
        "size_bytes": csv_bytes.len(),
        "expires_unix": expires_unix,
        "handle": handle,
        "row_count": rows.len(),
    }))
}

/// AC8: the bundle contains exactly this subject's `tenant_state_kv`/
/// `tenant_state_rows` rows (`state_kv/<key>.json`, `state_rows/<table>/<id>.json`)
/// and its call history (`calls.jsonl`) -- nothing from any other subject,
/// since every query here is pushed down by `end_user_subject`.
async fn build_enduser_archive(
    state: &AppState,
    tenant: &Tenant,
    run_id: &str,
    subject: &str,
) -> Result<Value, AppError> {
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;

    let kv_rows = state.db.state_kv_list(tenant.id, None, 100_000, subject.to_string()).await?;
    let state_rows = state.db.state_rows_all_for_end_user(tenant.id, subject.to_string()).await?;
    let calls = state.db.list_calls_for_subject(tenant.id, subject.to_string(), None, 100_000).await?;

    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(encoder);
    add_entry(&mut builder, "manifest.json", &to_json_bytes(&json!({"subject": subject}))?)?;
    for (key, value_json, _updated_unix) in &kv_rows {
        add_entry(&mut builder, &format!("state_kv/{key}.json"), value_json.as_bytes())?;
    }
    for (table_name, id, row_json) in &state_rows {
        add_entry(&mut builder, &format!("state_rows/{table_name}/{id}.json"), row_json.as_bytes())?;
    }
    let calls_jsonl = calls
        .iter()
        .map(|(ts, tool, outcome, run_id)| {
            json!({"ts": ts, "tool": tool, "outcome": outcome, "run_id": run_id}).to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    add_entry(&mut builder, "calls.jsonl", calls_jsonl.as_bytes())?;
    let archive_bytes = builder
        .into_inner()
        .map_err(|e| AppError::Internal(format!("tar build: {e}")))?
        .finish()
        .map_err(|e| AppError::Internal(format!("gzip finish: {e}")))?;

    if archive_bytes.len() as i64 > plan.export_bytes_max {
        return Err(AppError::Structured {
            code: "export_too_large",
            message: format!(
                "export archive is {} bytes, over this plan's {} byte limit",
                archive_bytes.len(),
                plan.export_bytes_max
            ),
            data: json!({
                "size_bytes": archive_bytes.len(),
                "limit_bytes": plan.export_bytes_max,
            }),
        });
    }

    let exports_dir = state.db.data_dir().join(EXPORT_DIR);
    tokio::fs::create_dir_all(&exports_dir)
        .await
        .map_err(|e| AppError::Storage(format!("create exports dir: {e}")))?;
    let archive_path = exports_dir.join(format!("{run_id}.tar.gz"));
    tokio::fs::write(&archive_path, &archive_bytes)
        .await
        .map_err(|e| AppError::Storage(format!("write export archive: {e}")))?;

    let expires_unix = crate::state::now_unix() + EXPORT_URL_TTL_SECS;
    let download_url = signed_download_url(&state.public_url, &tenant.key_hash, run_id, expires_unix);

    Ok(json!({
        "download_url": download_url,
        "size_bytes": archive_bytes.len(),
        "expires_unix": expires_unix,
        "subject": subject,
    }))
}

fn to_json_bytes(value: &Value) -> Result<Vec<u8>, AppError> {
    serde_json::to_vec_pretty(value).map_err(|e| AppError::Internal(format!("json serialize: {e}")))
}

fn add_entry(
    builder: &mut Builder<GzEncoder<Vec<u8>>>,
    path: &str,
    data: &[u8],
) -> Result<(), AppError> {
    let mut header = Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, path, data)
        .map_err(|e| AppError::Internal(format!("tar append {path}: {e}")))
}

/// PRD-mcphost-export-tables-and-import-restore R6: the one list of archive
/// entry kinds. The exporter renders every path through [`Self::path`], the
/// importer accepts a path only if [`Self::parse`] reads it, and
/// `docs/tools.md`'s archive-layout table is rendered from [`Self::layout`]
/// -- so the three cannot drift (`timp_ac07`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveEntry {
    Manifest,
    ToolConfig { name: String },
    ToolSource { name: String },
    State { key: String },
    Document { id: String },
    Secrets,
    Runs,
    Threads,
    Usage,
    TableSchema { name: String },
    TableRows { name: String },
    Model { table: String },
    Doc { name: String },
    DocMeta { name: String },
}

impl ArchiveEntry {
    pub fn path(&self) -> String {
        match self {
            ArchiveEntry::Manifest => "manifest.json".to_string(),
            ArchiveEntry::ToolConfig { name } => format!("tools/{name}/v1/config.json"),
            ArchiveEntry::ToolSource { name } => format!("tools/{name}/v1/source"),
            ArchiveEntry::State { key } => format!("state/{key}.json"),
            ArchiveEntry::Document { id } => format!("documents/{id}.json"),
            ArchiveEntry::Secrets => "secrets.txt".to_string(),
            ArchiveEntry::Runs => "runs.jsonl".to_string(),
            ArchiveEntry::Threads => "threads.jsonl".to_string(),
            ArchiveEntry::Usage => "usage.json".to_string(),
            ArchiveEntry::TableSchema { name } => format!("tables/{name}/schema.json"),
            ArchiveEntry::TableRows { name } => format!("tables/{name}/rows.jsonl"),
            ArchiveEntry::Model { table } => format!("models/{table}.json"),
            ArchiveEntry::Doc { name } => format!("docs/{name}"),
            ArchiveEntry::DocMeta { name } => format!("docs/{name}.meta.json"),
        }
    }

    /// The importer's reader: `Some` iff `path` is a member of the layout.
    pub fn parse(path: &str) -> Option<ArchiveEntry> {
        let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_string());
        match path {
            "manifest.json" => return Some(ArchiveEntry::Manifest),
            "secrets.txt" => return Some(ArchiveEntry::Secrets),
            "runs.jsonl" => return Some(ArchiveEntry::Runs),
            "threads.jsonl" => return Some(ArchiveEntry::Threads),
            "usage.json" => return Some(ArchiveEntry::Usage),
            _ => {}
        }
        if let Some(rest) = path.strip_prefix("tools/") {
            let (name, tail) = rest.split_once('/')?;
            let name = non_empty(name)?;
            return match tail {
                "v1/config.json" => Some(ArchiveEntry::ToolConfig { name }),
                "v1/source" => Some(ArchiveEntry::ToolSource { name }),
                _ => None,
            };
        }
        if let Some(rest) = path.strip_prefix("state/") {
            return Some(ArchiveEntry::State { key: non_empty(rest.strip_suffix(".json")?)? });
        }
        if let Some(rest) = path.strip_prefix("documents/") {
            return Some(ArchiveEntry::Document { id: non_empty(rest.strip_suffix(".json")?)? });
        }
        if let Some(rest) = path.strip_prefix("tables/") {
            let (name, tail) = rest.split_once('/')?;
            let name = non_empty(name)?;
            return match tail {
                "schema.json" => Some(ArchiveEntry::TableSchema { name }),
                "rows.jsonl" => Some(ArchiveEntry::TableRows { name }),
                _ => None,
            };
        }
        if let Some(rest) = path.strip_prefix("models/") {
            return Some(ArchiveEntry::Model { table: non_empty(rest.strip_suffix(".json")?)? });
        }
        if let Some(rest) = path.strip_prefix("docs/") {
            return match rest.strip_suffix(".meta.json") {
                Some(name) => Some(ArchiveEntry::DocMeta { name: non_empty(name)? }),
                None => Some(ArchiveEntry::Doc { name: non_empty(rest)? }),
            };
        }
        None
    }

    /// One representative of every variant, with `<placeholder>` names.
    fn samples() -> Vec<ArchiveEntry> {
        let n = || "<name>".to_string();
        vec![
            ArchiveEntry::Manifest,
            ArchiveEntry::ToolConfig { name: n() },
            ArchiveEntry::ToolSource { name: n() },
            ArchiveEntry::State { key: "<key>".to_string() },
            ArchiveEntry::Document { id: "<id>".to_string() },
            ArchiveEntry::Secrets,
            ArchiveEntry::Runs,
            ArchiveEntry::Threads,
            ArchiveEntry::Usage,
            ArchiveEntry::TableSchema { name: n() },
            ArchiveEntry::TableRows { name: n() },
            ArchiveEntry::Model { table: "<table>".to_string() },
            ArchiveEntry::Doc { name: n() },
            ArchiveEntry::DocMeta { name: n() },
        ]
    }

    /// What `host.import` does with this kind of entry. An exhaustive
    /// match: a new variant does not compile until it says.
    fn restore_note(&self) -> &'static str {
        match self {
            ArchiveEntry::Manifest => "read first: version, per-entry sha256 and bytes, tool kinds",
            ArchiveEntry::ToolConfig { .. } => "re-published through host.tool_publish",
            ArchiveEntry::ToolSource { .. } => "counted; the source is inside config.json",
            ArchiveEntry::State { .. } => "restored with host.state.set",
            ArchiveEntry::Document { .. } => "history only, not restored (docs/ is)",
            ArchiveEntry::Secrets => "names reported as secrets_to_set, never values",
            ArchiveEntry::Runs => "history only, not restored",
            ArchiveEntry::Threads => "history only, not restored",
            ArchiveEntry::Usage => "history only, not restored",
            ArchiveEntry::TableSchema { .. } => "restored with host.table.create",
            ArchiveEntry::TableRows { .. } => "restored with host.table.append, batches of 200",
            ArchiveEntry::Model { .. } => "restored with host.table.model_set",
            ArchiveEntry::Doc { .. } => "restored with host.docs.put (the index rebuilds)",
            ArchiveEntry::DocMeta { .. } => "mime and metadata for the doc beside it",
        }
    }

    /// This entry's kind as a layout pattern (`tools/<name>/v1/config.json`),
    /// the names replaced by the placeholders [`Self::samples`] uses.
    pub fn layout_pattern(&self) -> String {
        let n = || "<name>".to_string();
        match self {
            ArchiveEntry::ToolConfig { .. } => ArchiveEntry::ToolConfig { name: n() },
            ArchiveEntry::ToolSource { .. } => ArchiveEntry::ToolSource { name: n() },
            ArchiveEntry::State { .. } => ArchiveEntry::State { key: "<key>".to_string() },
            ArchiveEntry::Document { .. } => ArchiveEntry::Document { id: "<id>".to_string() },
            ArchiveEntry::TableSchema { .. } => ArchiveEntry::TableSchema { name: n() },
            ArchiveEntry::TableRows { .. } => ArchiveEntry::TableRows { name: n() },
            ArchiveEntry::Model { .. } => ArchiveEntry::Model { table: "<table>".to_string() },
            ArchiveEntry::Doc { .. } => ArchiveEntry::Doc { name: n() },
            ArchiveEntry::DocMeta { .. } => ArchiveEntry::DocMeta { name: n() },
            other => other.clone(),
        }
        .path()
    }

    /// `(path pattern, import behaviour)` for every kind, in layout order.
    pub fn layout() -> Vec<(String, &'static str)> {
        Self::samples().into_iter().map(|e| (e.path(), e.restore_note())).collect()
    }
}

/// `{path, sha256, bytes}` for one entry, as `manifest.json` lists it.
fn manifest_entry(path: &str, data: &[u8]) -> Value {
    json!({"path": path, "sha256": crate::billing::sha256_hex(data), "bytes": data.len()})
}

/// Everything one archive holds, gathered before any byte is written.
struct ArchiveContents<'a> {
    manifest_tools: &'a [Value],
    tools: &'a [ToolRow],
    secret_names: &'a [String],
    state_rows: &'a [(String, String, i64)],
    documents: &'a [(String, String, i64, String)],
    run_rows: &'a [RunRow],
    messages: &'a [MessageRow],
    usage: &'a Value,
    tables: &'a [crate::tables::TableDump],
    models: &'a [(String, Value)],
    docs: &'a [DocFile],
}

/// One live document as `docs/<name>` + `docs/<name>.meta.json` carry it.
struct DocFile {
    name: String,
    content: Vec<u8>,
    meta: Value,
}

fn jsonl(lines: impl Iterator<Item = String>) -> Vec<u8> {
    let mut out = lines.collect::<Vec<_>>().join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.into_bytes()
}

/// Requirement 1: the archive layout -- see [`ArchiveEntry`] for every path.
/// `manifest.json` (version 2) is written first and lists every other entry
/// with its `sha256` and `bytes`; `tools/<name>/v1/source` exists only for a
/// kind whose spec carries a `source` string, `documents/<id>.json` is the
/// pre-v2 text view of each live document (docs/ carries the original
/// bytes), `secrets.txt` is names only, and `runs.jsonl`/`threads.jsonl`
/// are metadata.
fn build_tar_gz(c: &ArchiveContents<'_>) -> Result<Vec<u8>, AppError> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut push = |entry: ArchiveEntry, data: Vec<u8>| entries.push((entry.path(), data));

    for tool in c.tools {
        let name = tool.name.clone();
        push(ArchiveEntry::ToolConfig { name: name.clone() }, to_json_bytes(&tool.spec)?);
        // AC1's "both tool sources": only kinds whose spec carries a
        // `source` string (e.g. `python`) get a `source` file -- a kind
        // with no such concept (`echo`) is fully represented by
        // `config.json` alone.
        if let Some(source) = tool.spec.get("source").and_then(Value::as_str) {
            push(ArchiveEntry::ToolSource { name }, source.as_bytes().to_vec());
        }
    }
    for (key, value_json, _updated_unix) in c.state_rows {
        push(ArchiveEntry::State { key: key.clone() }, value_json.as_bytes().to_vec());
    }
    // AC11: one file per live document, its current version's id/name/
    // version/extracted text.
    for (id, name, version, text) in c.documents {
        let doc_json = to_json_bytes(&json!({"id": id, "name": name, "version": version, "text": text}))?;
        push(ArchiveEntry::Document { id: id.clone() }, doc_json);
    }
    // AC1: names only, never values -- `secret_names` already comes from
    // `list_secret_names`, which never touches `value_enc`/`nonce`.
    push(ArchiveEntry::Secrets, c.secret_names.join("\n").into_bytes());
    push(
        ArchiveEntry::Runs,
        jsonl(c.run_rows.iter().map(|r| {
            json!({
                "id": r.id,
                "tool": r.tool_name,
                "trigger": r.trigger,
                "status": r.status,
                "started_unix": r.started_unix,
                "finished_unix": r.finished_unix,
                "duration_ms": r.duration_ms,
                "error_class": r.error_class,
            })
            .to_string()
        })),
    );
    push(
        ArchiveEntry::Threads,
        jsonl(c.messages.iter().map(|m| {
            json!({
                "thread_id": m.thread_id,
                "seq": m.seq,
                "from": m.from_address,
                "body": m.body,
                "data": m.data,
                "created_at": m.created_at,
            })
            .to_string()
        })),
    );
    push(ArchiveEntry::Usage, to_json_bytes(c.usage)?);
    // R1: schema before rows, so the importer can create the table first.
    for table in c.tables {
        push(ArchiveEntry::TableSchema { name: table.name.clone() }, to_json_bytes(&table.schema)?);
        push(ArchiveEntry::TableRows { name: table.name.clone() }, jsonl(table.rows.iter().map(Value::to_string)));
    }
    for (table, notes) in c.models {
        push(ArchiveEntry::Model { table: table.clone() }, to_json_bytes(notes)?);
    }
    for doc in c.docs {
        push(ArchiveEntry::DocMeta { name: doc.name.clone() }, to_json_bytes(&doc.meta)?);
        push(ArchiveEntry::Doc { name: doc.name.clone() }, doc.content.clone());
    }

    let manifest = to_json_bytes(&json!({
        "version": MANIFEST_VERSION,
        "tools": c.manifest_tools,
        "entries": entries.iter().map(|(p, d)| manifest_entry(p, d)).collect::<Vec<_>>(),
        "total_bytes": entries.iter().map(|(_, d)| d.len()).sum::<usize>(),
    }))?;

    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(encoder);
    add_entry(&mut builder, &ArchiveEntry::Manifest.path(), &manifest)?;
    for (path, data) in &entries {
        add_entry(&mut builder, path, data)?;
    }
    let encoder = builder
        .into_inner()
        .map_err(|e| AppError::Internal(format!("tar build: {e}")))?;
    encoder
        .finish()
        .map_err(|e| AppError::Internal(format!("gzip finish: {e}")))
}

/// `manifest.json`'s `version` for archives this build writes.
pub const MANIFEST_VERSION: i64 = 2;

/// Gathers this tenant's exportable data, builds the archive, writes it
/// under this host's data dir (technical considerations: "the tenant's
/// scratch quota"), and returns the `runs.get` result value (download URL,
/// size, manifest).
async fn build_archive(
    state: &AppState,
    tenant: &Tenant,
    run_id: &str,
    tools_filter: Option<&[String]>,
) -> Result<Value, AppError> {
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;

    let mut tools = state.db.list_tools(tenant.id).await?;
    // P1 requirement 5 / AC6: `{tools: ["a"]}` exports only tool `a`.
    if let Some(filter) = tools_filter {
        tools.retain(|t| filter.iter().any(|name| name == &t.name));
    }
    let secret_names = state.db.list_secret_names(tenant.id).await?;
    // `runs/<id>/...` keys are the host's own result parts and import
    // progress, already summarised by `runs.jsonl` -- not tenant state.
    let state_rows: Vec<_> = state
        .db
        .state_kv_list(tenant.id, None, 100_000, String::new())
        .await?
        .into_iter()
        .filter(|(key, _, _)| !key.starts_with(crate::runs::RUN_KEY_PREFIX))
        .collect();
    let documents = state.db.documents_for_export(tenant.id).await?;
    // PRD-mcphost-chain-run-lineage: unchanged export scope -- top-level
    // runs only (`parent_run_id: None, include_children: false`), same set
    // this export already archived before composed child rows existed.
    let run_rows = state.db.list_runs(tenant.id, None, None, None, None, None, false, 1000).await?;
    let messages = state.db.msg_inbox(tenant.id, None, 1000, false).await?;
    let usage = crate::control::usage(state, tenant, &json!({})).await?;

    let tables = crate::tables::dump_tables(state, tenant).await?;
    let mut models: Vec<(String, Value)> = Vec::new();
    for table in &tables {
        let mut notes = state.db.list_table_model_annotations(tenant.id, table.name.clone()).await?;
        notes.sort_by(|a, b| (&a.column_name, &a.key).cmp(&(&b.column_name, &b.key)));
        if !notes.is_empty() {
            let notes: Vec<Value> = notes
                .iter()
                .map(|n| json!({"column": n.column_name, "key": n.key, "value": n.value}))
                .collect();
            models.push((table.name.clone(), json!(notes)));
        }
    }
    let mut docs: Vec<DocFile> = Vec::new();
    for doc in state.db.documents_list_live(tenant.id, None, None, crate::paged::DocsOrder::NameAsc, 100_000).await? {
        let Some((content, _text)) = state.db.document_blob_get(tenant.id, doc.id.clone(), doc.version).await? else {
            continue;
        };
        let metadata: Value = serde_json::from_str(&doc.metadata_json).unwrap_or(Value::Null);
        docs.push(DocFile {
            name: doc.name.clone(),
            content,
            meta: json!({"mime": doc.mime, "bytes": doc.bytes, "metadata": metadata}),
        });
    }

    // P1 requirement 4 / AC5: each manifest entry is exactly a
    // `host.tool_publish` argument object -- see
    // `handler::tool_publish_input_schema` for the schema it validates
    // against.
    let manifest: Vec<Value> = tools
        .iter()
        .map(|t| json!({"name": t.name, "kind": t.kind, "spec": t.spec}))
        .collect();

    let archive_bytes = build_tar_gz(&ArchiveContents {
        manifest_tools: &manifest,
        tools: &tools,
        secret_names: &secret_names,
        state_rows: &state_rows,
        documents: &documents,
        run_rows: &run_rows,
        messages: &messages,
        usage: &usage,
        tables: &tables,
        models: &models,
        docs: &docs,
    })?;

    if archive_bytes.len() as i64 > plan.export_bytes_max {
        return Err(AppError::Structured {
            code: "export_too_large",
            message: format!(
                "export archive is {} bytes, over this plan's {} byte limit",
                archive_bytes.len(),
                plan.export_bytes_max
            ),
            data: json!({
                "size_bytes": archive_bytes.len(),
                "limit_bytes": plan.export_bytes_max,
            }),
        });
    }

    let exports_dir = state.db.data_dir().join(EXPORT_DIR);
    tokio::fs::create_dir_all(&exports_dir)
        .await
        .map_err(|e| AppError::Storage(format!("create exports dir: {e}")))?;
    let archive_path = exports_dir.join(format!("{run_id}.tar.gz"));
    tokio::fs::write(&archive_path, &archive_bytes)
        .await
        .map_err(|e| AppError::Storage(format!("write export archive: {e}")))?;

    // P0 requirement 2: valid 24h.
    let expires_unix = crate::state::now_unix() + EXPORT_URL_TTL_SECS;
    let download_url = signed_download_url(&state.public_url, &tenant.key_hash, run_id, expires_unix);

    Ok(json!({
        "download_url": download_url,
        "size_bytes": archive_bytes.len(),
        "expires_unix": expires_unix,
        "manifest": manifest,
    }))
}

/// `GET /exports/{run_id}?expires=<unix>&sig=<hex>` (P0 requirement 2):
/// unauthenticated -- the signature itself is the whole auth story, same
/// shape as `host.redeem`'s handoff token. AC2: downloads within 24h of
/// the export completing, 410 after.
pub async fn download(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Ok(Some(run)) = state.db.find_run_by_id(run_id.clone()).await else {
        return (StatusCode::NOT_FOUND, "export not found").into_response();
    };
    // PRD-mcphost-result-handles P1 requirement 7: a handle export is a
    // `.csv` file, `text/csv` -- the other two kinds are always the
    // `.tar.gz`/`application/gzip` bundle `build_archive`/
    // `build_enduser_archive` write.
    let is_handle_export = run.tool_name == HANDLE_EXPORT_TOOL_NAME;
    let is_export = run.tool_name == EXPORT_TOOL_NAME || run.tool_name == ENDUSER_EXPORT_TOOL_NAME || is_handle_export;
    if !is_export || run.status != "done" {
        return (StatusCode::NOT_FOUND, "export not found").into_response();
    }
    let Ok(Some(tenant)) = state.db.find_tenant_by_id(run.tenant_id).await else {
        return (StatusCode::NOT_FOUND, "export not found").into_response();
    };
    // AC2: 403 for a missing/altered signature, 410 for a genuine one past
    // its expiry -- one shared check (R5).
    if let Err((status, msg)) = crate::signed_url::verify(
        &tenant.key_hash,
        &export_path(&run_id),
        params.get("expires").map(String::as_str),
        params.get("sig").map(String::as_str),
        crate::state::now_unix(),
    ) {
        return (status, msg).into_response();
    }
    let (filename, content_type) = if is_handle_export {
        (format!("{run_id}.csv"), "text/csv")
    } else {
        (format!("{run_id}.tar.gz"), "application/gzip")
    };
    let archive_path = state.db.data_dir().join(EXPORT_DIR).join(&filename);
    match tokio::fs::read(&archive_path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                ("content-type", content_type),
                (
                    "content-disposition",
                    if is_handle_export {
                        "attachment; filename=\"export.csv\""
                    } else {
                        "attachment; filename=\"export.tar.gz\""
                    },
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "export archive missing").into_response(),
    }
}
