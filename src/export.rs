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
/// PRD-mcphost-org-export-and-offboard-v2 R1: a fourth distinct
/// `runs.tool_name`, so an org export's own "one running at a time" check
/// never meets a tenant export (the reasoning of [`ENDUSER_EXPORT_TOOL_NAME`]).
/// [`download`] serves its bundle as a `.tar.gz`.
pub const ORG_EXPORT_TOOL_NAME: &str = "host.org.export";
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

/// R2: `include_blobs: true` adds each blob's bytes at `blobs/<id>`; absent
/// or `false`, the archive carries `blobs/index.json` only.
fn arg_include_blobs(args: &Value) -> Result<bool, AppError> {
    match args.get("include_blobs") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(AppError::InvalidArgs("include_blobs: must be a boolean".to_string())),
    }
}

/// `host.export {tools?: [name, ...]}` (P0 requirements 1-3): starts (or,
/// per requirement 3 / AC3, rejoins) a background job that builds this
/// tenant's whole data archive. Returns immediately -- the archive itself
/// is built by [`run_export_job`], spawned below, and finalized into the
/// same `runs` ledger every other async job uses.
pub async fn export(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let tools_filter = arg_tools_filter(args)?;
    let include_blobs = arg_include_blobs(args)?;
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    // R2/AC5: the blob bytes alone are checked against the plan's
    // `export_bytes_max` before a run row exists or any blob file is read --
    // the form `host.import` refuses with (`import.rs`).
    if include_blobs {
        let (_blobs, blob_bytes) = state.db.blobs_usage(tenant.id).await?;
        if blob_bytes > plan.export_bytes_max {
            return Err(crate::billing::quota_exceeded_for(
                &state.plans,
                state.billing_config.billing_mode(),
                &tenant.plan,
                "export_bytes_max",
                plan.export_bytes_max,
                blob_bytes,
                None,
            ));
        }
    }
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
    // R4: an export already `running` for this tenant is answered with a
    // typed `export_in_progress` carrying that run's id and its progress,
    // not a second job.
    let run_id = match outcome {
        StartExportRun::AlreadyRunning(existing) => return Err(export_in_progress(state, tenant, &existing).await?),
        StartExportRun::Started(new_id) => new_id,
    };
    let spawn_state = state.clone();
    let spawn_tenant = tenant.clone();
    let spawn_run_id = run_id.clone();
    tokio::spawn(async move {
        run_export_job(spawn_state, spawn_tenant, spawn_run_id, tools_filter, include_blobs).await;
    });
    Ok(json!({"run_id": run_id, "status": "running"}))
}

/// The `export_in_progress` refusal for the export `run_id` already running:
/// `{run_id, started_unix, entries_done, entries_total}`, the counters read
/// from the run's progress key (`entries_total` is `null` until the collector
/// has counted, and `entries_done` is 0).
async fn export_in_progress(state: &AppState, tenant: &Tenant, run_id: &str) -> Result<AppError, AppError> {
    let started_unix = state.db.get_run(run_id.to_string(), tenant.id).await?.map(|r| r.started_unix);
    let progress = match state
        .db
        .state_kv_get(tenant.id, crate::import::progress_key(run_id), String::new())
        .await?
    {
        Some((json, _)) => serde_json::from_str::<Value>(&json).unwrap_or(Value::Null),
        None => Value::Null,
    };
    Ok(AppError::Structured {
        code: EXPORT_IN_PROGRESS,
        message: format!("an export is already running for this tenant (run {run_id}); poll host.runs.get with that run_id"),
        data: json!({
            "run_id": run_id,
            "started_unix": started_unix,
            "entries_done": progress.get("entries_done").cloned().unwrap_or(json!(0)),
            "entries_total": progress.get("entries_total").cloned().unwrap_or(Value::Null),
        }),
    })
}

/// Runs one export to completion and finalizes its `runs` row -- same
/// "never panic out of the spawn, degrade to an error instead" shape as
/// `runs::run_one_job`.
async fn run_export_job(
    state: AppState,
    tenant: Tenant,
    run_id: String,
    tools_filter: Option<Vec<String>>,
    include_blobs: bool,
) {
    let start = std::time::Instant::now();
    let outcome = build_archive(&state, &tenant, &run_id, tools_filter.as_deref(), include_blobs).await;
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
            code: EXPORT_TOO_LARGE,
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

pub(crate) fn to_json_bytes(value: &Value) -> Result<Vec<u8>, AppError> {
    serde_json::to_vec_pretty(value).map_err(|e| AppError::Internal(format!("json serialize: {e}")))
}

pub(crate) fn add_entry(
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
    Calls,
    TenantAudit,
    AuditChain,
    OauthAudit,
    Triggers,
    BlobIndex,
    Blob { id: String },
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
            ArchiveEntry::Calls => "calls.jsonl".to_string(),
            ArchiveEntry::TenantAudit => "tenant_audit.jsonl".to_string(),
            ArchiveEntry::AuditChain => "audit_chain.jsonl".to_string(),
            ArchiveEntry::OauthAudit => "oauth_audit.jsonl".to_string(),
            ArchiveEntry::Triggers => "triggers.jsonl".to_string(),
            ArchiveEntry::BlobIndex => "blobs/index.json".to_string(),
            ArchiveEntry::Blob { id } => format!("blobs/{id}"),
        }
    }

    /// The main-db tables this kind of entry is read from -- the covered set
    /// the schema walk ([`uncovered_tenant_tables`]) checks `EXPORT_EXCLUSIONS`
    /// against. An exhaustive match, like [`Self::restore_note`]: a new variant
    /// does not compile until it names its tables. `TableSchema`/`TableRows`
    /// read the tenant's own table file, and `Usage` is an aggregate computed
    /// from several tables, so they name none.
    pub fn source_tables(&self) -> &'static [&'static str] {
        match self {
            ArchiveEntry::Manifest => &[],
            ArchiveEntry::ToolConfig { .. } | ArchiveEntry::ToolSource { .. } => &["tools"],
            ArchiveEntry::State { .. } => &["tenant_state_kv"],
            ArchiveEntry::Document { .. } | ArchiveEntry::Doc { .. } | ArchiveEntry::DocMeta { .. } => {
                &["documents", "document_blobs"]
            }
            ArchiveEntry::Secrets => &["secrets"],
            ArchiveEntry::Runs => &["runs"],
            ArchiveEntry::Threads => &["messages", "message_receipts"],
            ArchiveEntry::Usage => &[],
            ArchiveEntry::TableSchema { .. } | ArchiveEntry::TableRows { .. } => &[],
            ArchiveEntry::Model { .. } => &["table_model_annotations"],
            ArchiveEntry::Calls => &["calls"],
            ArchiveEntry::TenantAudit => &["tenant_audit"],
            ArchiveEntry::AuditChain => &["audit_chain"],
            ArchiveEntry::OauthAudit => &["oauth_audit"],
            ArchiveEntry::Triggers => &["triggers"],
            ArchiveEntry::BlobIndex | ArchiveEntry::Blob { .. } => &["blobs"],
        }
    }

    /// The data class this entry belongs to, as `manifest.json`'s
    /// `data_classes` names it: one class per variant (`ToolConfig` and
    /// `ToolSource` are two).
    pub fn class_name(&self) -> &'static str {
        match self {
            ArchiveEntry::Manifest => "manifest",
            ArchiveEntry::ToolConfig { .. } => "tool_config",
            ArchiveEntry::ToolSource { .. } => "tool_source",
            ArchiveEntry::State { .. } => "state",
            ArchiveEntry::Document { .. } => "document",
            ArchiveEntry::Secrets => "secrets",
            ArchiveEntry::Runs => "runs",
            ArchiveEntry::Threads => "threads",
            ArchiveEntry::Usage => "usage",
            ArchiveEntry::TableSchema { .. } => "table_schema",
            ArchiveEntry::TableRows { .. } => "table_rows",
            ArchiveEntry::Model { .. } => "model",
            ArchiveEntry::Doc { .. } => "doc",
            ArchiveEntry::DocMeta { .. } => "doc_meta",
            ArchiveEntry::Calls => "calls",
            ArchiveEntry::TenantAudit => "tenant_audit",
            ArchiveEntry::AuditChain => "audit_chain",
            ArchiveEntry::OauthAudit => "oauth_audit",
            ArchiveEntry::Triggers => "triggers",
            ArchiveEntry::BlobIndex => "blob_index",
            ArchiveEntry::Blob { .. } => "blob",
        }
    }

    /// One representative of every class the manifest reports (every
    /// variant but `Manifest`).
    pub fn classes() -> Vec<ArchiveEntry> {
        Self::samples().into_iter().filter(|e| *e != ArchiveEntry::Manifest).collect()
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
            "calls.jsonl" => return Some(ArchiveEntry::Calls),
            "tenant_audit.jsonl" => return Some(ArchiveEntry::TenantAudit),
            "audit_chain.jsonl" => return Some(ArchiveEntry::AuditChain),
            "oauth_audit.jsonl" => return Some(ArchiveEntry::OauthAudit),
            "triggers.jsonl" => return Some(ArchiveEntry::Triggers),
            "blobs/index.json" => return Some(ArchiveEntry::BlobIndex),
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
        if let Some(rest) = path.strip_prefix("blobs/") {
            return Some(ArchiveEntry::Blob { id: non_empty(rest)? });
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
            ArchiveEntry::Calls,
            ArchiveEntry::TenantAudit,
            ArchiveEntry::AuditChain,
            ArchiveEntry::OauthAudit,
            ArchiveEntry::Triggers,
            ArchiveEntry::BlobIndex,
            ArchiveEntry::Blob { id: "<id>".to_string() },
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
            ArchiveEntry::Calls
            | ArchiveEntry::TenantAudit
            | ArchiveEntry::AuditChain
            | ArchiveEntry::OauthAudit
            | ArchiveEntry::Triggers
            | ArchiveEntry::BlobIndex
            | ArchiveEntry::Blob { .. } => "history only, not restored",
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
            ArchiveEntry::Blob { .. } => ArchiveEntry::Blob { id: "<id>".to_string() },
            other => other.clone(),
        }
        .path()
    }

    /// `(path pattern, import behaviour)` for every kind, in layout order.
    pub fn layout() -> Vec<(String, &'static str)> {
        Self::samples().into_iter().map(|e| (e.path(), e.restore_note())).collect()
    }
}

/// PRD-mcphost-export-data-class-completeness-v2 R1: every table of a
/// migrated db that scopes its rows to a tenant -- a column named
/// `tenant_id` or ending in `_tenant_id` (`messages` scopes by
/// `from_tenant_id` only) -- sorted. The schema, not a list, is the valid set.
pub fn tenant_scoped_tables(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    let mut names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    names.retain(|table| {
        let quoted = table.replace('"', "\"\"");
        let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info(\"{quoted}\")")) else {
            return false;
        };
        let Ok(cols) = stmt.query_map([], |r| r.get::<_, String>(1)) else {
            return false;
        };
        cols.flatten().any(|c| c == "tenant_id" || c.ends_with("_tenant_id"))
    });
    Ok(names)
}

/// The completeness walk: the tenant-scoped tables of `conn` that are
/// neither in `covered` (an archive class's source tables) nor in `excluded`
/// (a documented exclusion). Empty means the archive accounts for every
/// table; a test failure message names what is left.
pub fn uncovered_tenant_tables(conn: &rusqlite::Connection, covered: &[&str], excluded: &[&str]) -> Vec<String> {
    tenant_scoped_tables(conn)
        .expect("schema walk reads sqlite_master")
        .into_iter()
        .filter(|t| !covered.contains(&t.as_str()) && !excluded.contains(&t.as_str()))
        .collect()
}

/// Why a tenant-scoped table is not an archive class. Each renders to the
/// reason text `manifest.json`'s `excluded` list and `docs/tools.md` carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exclusion {
    /// An operational record of the host, not data the tenant gave it.
    HostInternal,
    /// The host's own billing record of the tenant.
    BillingRecord,
    /// Rebuilt from archived data on demand.
    Derived,
    /// A credential or other secret material: never exported.
    SecretMaterial,
    /// Carried inside another entry kind (named by its path pattern).
    CoveredBy(&'static str),
    /// Tenant-held data no archive class carries yet: stated in the manifest
    /// rather than left out silently, and the next classes the walk is for.
    NotYetArchived,
}

impl Exclusion {
    pub fn reason(&self) -> String {
        match self {
            Exclusion::HostInternal => "host-internal operational record".to_string(),
            Exclusion::BillingRecord => "billing record, retained by host".to_string(),
            Exclusion::Derived => "derived from archived data, rebuilt on demand".to_string(),
            Exclusion::SecretMaterial => "credential or secret material, never exported".to_string(),
            Exclusion::CoveredBy(entry) => format!("carried in {entry}"),
            Exclusion::NotYetArchived => "tenant-held, not yet archived".to_string(),
        }
    }
}

/// PRD-mcphost-export-data-class-completeness-v2 R1: every tenant-scoped
/// table that is not an [`ArchiveEntry`] source table, with its reason. The
/// drift test (`expc_ac02`) walks the migrated schema, so a new table with a
/// `tenant_id` column fails CI until it is an archive class or listed here.
pub const EXPORT_EXCLUSIONS: &[(&str, Exclusion)] = &[
    ("agent_profiles", Exclusion::NotYetArchived),
    ("asks", Exclusion::NotYetArchived),
    ("audit_exports", Exclusion::HostInternal),
    ("billing_events", Exclusion::BillingRecord),
    ("blob_policy", Exclusion::NotYetArchived),
    ("blob_versions", Exclusion::NotYetArchived),
    ("blocks", Exclusion::NotYetArchived),
    ("call_results", Exclusion::Derived),
    ("call_traces", Exclusion::NotYetArchived),
    ("catalog_receipts", Exclusion::HostInternal),
    ("channel_cursors", Exclusion::HostInternal),
    ("channel_posts", Exclusion::NotYetArchived),
    ("chart_index", Exclusion::Derived),
    ("claim_codes", Exclusion::SecretMaterial),
    ("claim_email_events", Exclusion::HostInternal),
    ("confirm_tokens", Exclusion::SecretMaterial),
    ("consent_cookies", Exclusion::SecretMaterial),
    ("contact_requests", Exclusion::NotYetArchived),
    ("contacts", Exclusion::NotYetArchived),
    ("context_versions", Exclusion::NotYetArchived),
    ("dead_letter_drops", Exclusion::HostInternal),
    ("dead_letters", Exclusion::NotYetArchived),
    ("doc_chunks", Exclusion::CoveredBy("docs/<name>")),
    ("doc_chunks_fts", Exclusion::CoveredBy("docs/<name>")),
    ("doc_index_state", Exclusion::CoveredBy("docs/<name>")),
    ("document_usage_events", Exclusion::HostInternal),
    ("draft_events", Exclusion::HostInternal),
    ("drift_alerts", Exclusion::Derived),
    ("drift_queue", Exclusion::Derived),
    ("drift_reviews", Exclusion::Derived),
    ("email_sends", Exclusion::NotYetArchived),
    ("emitted_reviews", Exclusion::Derived),
    ("end_user_attributes", Exclusion::NotYetArchived),
    ("end_users", Exclusion::NotYetArchived),
    ("example_proposals", Exclusion::NotYetArchived),
    ("feedback", Exclusion::NotYetArchived),
    ("group_members", Exclusion::NotYetArchived),
    ("groups", Exclusion::NotYetArchived),
    ("handoff_tokens", Exclusion::SecretMaterial),
    ("help_search_events", Exclusion::HostInternal),
    ("hint_events", Exclusion::HostInternal),
    ("host_tool_usage", Exclusion::HostInternal),
    ("index_chunks", Exclusion::Derived),
    ("index_sources", Exclusion::NotYetArchived),
    ("indexes", Exclusion::NotYetArchived),
    ("invite_joins", Exclusion::NotYetArchived),
    ("invites", Exclusion::SecretMaterial),
    ("join_codes", Exclusion::SecretMaterial),
    ("lineage_edges", Exclusion::Derived),
    ("lineage_nodes", Exclusion::Derived),
    ("logs", Exclusion::NotYetArchived),
    ("meter_events", Exclusion::BillingRecord),
    ("muted", Exclusion::NotYetArchived),
    ("network_denials", Exclusion::HostInternal),
    ("notifies", Exclusion::NotYetArchived),
    ("notify_channels", Exclusion::SecretMaterial),
    ("notify_receipts", Exclusion::NotYetArchived),
    ("oauth_approved_clients", Exclusion::NotYetArchived),
    ("oauth_codes", Exclusion::SecretMaterial),
    ("oauth_denied_clients", Exclusion::NotYetArchived),
    ("oauth_federation_pending", Exclusion::SecretMaterial),
    ("oauth_grants", Exclusion::NotYetArchived),
    ("oauth_issuers", Exclusion::NotYetArchived),
    ("oauth_jti_denylist", Exclusion::HostInternal),
    ("oauth_pending_clients", Exclusion::NotYetArchived),
    ("oauth_policies", Exclusion::NotYetArchived),
    ("oauth_providers", Exclusion::SecretMaterial),
    ("oauth_scopes", Exclusion::NotYetArchived),
    ("oauth_trusted_issuers", Exclusion::NotYetArchived),
    ("org_members", Exclusion::NotYetArchived),
    ("orgs", Exclusion::NotYetArchived),
    ("owner_dns_challenges", Exclusion::SecretMaterial),
    ("owner_github_states", Exclusion::SecretMaterial),
    ("recipe_uses", Exclusion::HostInternal),
    ("registry_documents", Exclusion::NotYetArchived),
    ("row_policies", Exclusion::NotYetArchived),
    ("sampling_usage", Exclusion::HostInternal),
    ("signup_events", Exclusion::HostInternal),
    ("syncs", Exclusion::NotYetArchived),
    ("table_change_counters", Exclusion::Derived),
    ("table_graphs", Exclusion::Derived),
    ("table_models", Exclusion::Derived),
    ("tasks", Exclusion::NotYetArchived),
    ("tenant_clients", Exclusion::SecretMaterial),
    ("tenant_end_user_activity", Exclusion::HostInternal),
    ("tenant_keys", Exclusion::SecretMaterial),
    ("tenant_lifecycle_events", Exclusion::HostInternal),
    ("tenant_state_rows", Exclusion::NotYetArchived),
    ("tenant_state_tables", Exclusion::NotYetArchived),
    ("tenants", Exclusion::NotYetArchived),
    ("thread_participants", Exclusion::NotYetArchived),
    ("tool_caller_limits", Exclusion::NotYetArchived),
    ("tool_lock", Exclusion::Derived),
    ("tool_share_tokens", Exclusion::SecretMaterial),
    ("tool_version_watermarks", Exclusion::HostInternal),
    ("tool_versions", Exclusion::NotYetArchived),
    ("trigger_receipts", Exclusion::NotYetArchived),
    ("usage_daily", Exclusion::Derived),
    ("vault_handoff_tokens", Exclusion::SecretMaterial),
    ("vault_providers", Exclusion::SecretMaterial),
    ("vault_tokens", Exclusion::SecretMaterial),
];

/// PRD-mcphost-export-data-class-completeness-v2 R6: `docs/tools.md`'s
/// archive-layout table, rendered from [`ArchiveEntry::layout`].
pub fn render_layout_table() -> String {
    let mut out = String::from("| path | host.import |\n|---|---|\n");
    for (pattern, note) in ArchiveEntry::layout() {
        out.push_str(&format!("| `{pattern}` | {note} |\n"));
    }
    out
}

/// The heading `docs/tools.md`'s excluded-classes table sits under.
pub const EXCLUDED_CLASSES_HEADING: &str = "\n### Excluded classes\n";

/// R6: the excluded-classes table, rendered from [`EXPORT_EXCLUSIONS`].
pub fn render_exclusions_table() -> String {
    let mut out = String::from("| table | reason |\n|---|---|\n");
    for (table, why) in EXPORT_EXCLUSIONS {
        out.push_str(&format!("| `{table}` | {} |\n", why.reason()));
    }
    out
}

/// The table names of [`EXPORT_EXCLUSIONS`].
pub fn excluded_tables() -> Vec<&'static str> {
    EXPORT_EXCLUSIONS.iter().map(|(table, _)| *table).collect()
}

/// Every source table of every [`ArchiveEntry`] class.
pub fn covered_tables() -> Vec<&'static str> {
    let mut tables: Vec<&'static str> =
        ArchiveEntry::classes().iter().flat_map(|e| e.source_tables().iter().copied()).collect();
    tables.sort_unstable();
    tables.dedup();
    tables
}

/// `{path, sha256, bytes}` for one entry, as `manifest.json` lists it.
fn manifest_entry(path: &str, data: &[u8]) -> Value {
    json!({"path": path, "sha256": crate::billing::sha256_hex(data), "bytes": data.len()})
}

/// How many rows of data one entry holds: lines of a `.jsonl` or the names
/// in `secrets.txt`, the array length of the blob index, otherwise one.
fn rows_in(entry: &ArchiveEntry, data: &[u8]) -> usize {
    let lines = || data.split(|b| *b == b'\n').filter(|l| !l.is_empty()).count();
    match entry {
        ArchiveEntry::Secrets => lines(),
        ArchiveEntry::BlobIndex => serde_json::from_slice::<Vec<Value>>(data).map_or(0, |rows| rows.len()),
        _ if entry.path().ends_with(".jsonl") => lines(),
        _ => 1,
    }
}

/// R3: one `{class, table, entries, rows, sha256}` row per [`ArchiveEntry`]
/// class, in layout order. `sha256` is over the class's entry bytes
/// concatenated in path order, so an auditor re-derives it from the archive;
/// a class with no entry carries the sha256 of zero bytes.
fn data_classes(entries: &[(String, Vec<u8>)]) -> Vec<Value> {
    let parsed: Vec<(ArchiveEntry, &str, &[u8])> = entries
        .iter()
        .filter_map(|(path, data)| ArchiveEntry::parse(path).map(|e| (e, path.as_str(), data.as_slice())))
        .collect();
    ArchiveEntry::classes()
        .iter()
        .map(|class| {
            let mut members: Vec<&(ArchiveEntry, &str, &[u8])> =
                parsed.iter().filter(|(e, _, _)| e.class_name() == class.class_name()).collect();
            members.sort_by_key(|(_, path, _)| *path);
            let bytes: Vec<u8> = members.iter().flat_map(|(_, _, data)| data.iter().copied()).collect();
            json!({
                "class": class.class_name(),
                "table": class.source_tables().join(","),
                "entries": members.len(),
                "rows": members.iter().map(|(e, _, data)| rows_in(e, data)).sum::<usize>(),
                "sha256": crate::billing::sha256_hex(&bytes),
            })
        })
        .collect()
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
    calls: &'a [crate::calllog::CallRow],
    tenant_audit: &'a [Value],
    audit_chain: &'a [Value],
    oauth_audit: &'a [Value],
    triggers: &'a [Value],
    blobs: &'a [crate::db::BlobRow],
    /// `(id, bytes)` of each blob file, only for `include_blobs: true`.
    blob_files: &'a [(String, Vec<u8>)],
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
async fn build_tar_gz(
    state: &AppState,
    tenant: &Tenant,
    progress_run_id: Option<&str>,
    c: &ArchiveContents<'_>,
) -> Result<Vec<u8>, AppError> {
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
    // R2: the classes the schema walk surfaced. Each is written even when it
    // holds no rows, so the layout of an archive does not depend on the data.
    push(ArchiveEntry::Calls, jsonl(c.calls.iter().rev().map(|row| row.to_json().to_string())));
    push(ArchiveEntry::TenantAudit, jsonl(c.tenant_audit.iter().map(Value::to_string)));
    push(ArchiveEntry::AuditChain, jsonl(c.audit_chain.iter().map(Value::to_string)));
    push(ArchiveEntry::OauthAudit, jsonl(c.oauth_audit.iter().map(Value::to_string)));
    push(ArchiveEntry::Triggers, jsonl(c.triggers.iter().map(Value::to_string)));
    let blob_index: Vec<Value> = c
        .blobs
        .iter()
        .map(|b| json!({"id": b.id, "name": b.name, "version": b.version, "mime": b.mime, "bytes": b.bytes, "sha256": b.sha256}))
        .collect();
    push(ArchiveEntry::BlobIndex, to_json_bytes(&json!(blob_index))?);
    for (id, bytes) in c.blob_files {
        push(ArchiveEntry::Blob { id: id.clone() }, bytes.clone());
    }

    let manifest = to_json_bytes(&json!({
        "version": MANIFEST_VERSION,
        "tools": c.manifest_tools,
        "entries": entries.iter().map(|(p, d)| manifest_entry(p, d)).collect::<Vec<_>>(),
        "total_bytes": entries.iter().map(|(_, d)| d.len()).sum::<usize>(),
        "exported_unix": crate::state::now_unix(),
        "host_version": env!("CARGO_PKG_VERSION"),
        "data_classes": data_classes(&entries),
        "excluded": EXPORT_EXCLUSIONS
            .iter()
            .map(|(table, why)| json!({"table": table, "reason": why.reason()}))
            .collect::<Vec<_>>(),
    }))?;

    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(encoder);
    add_entry(&mut builder, &ArchiveEntry::Manifest.path(), &manifest)?;
    // R4: the count is known once the entries are collected; `host.runs.get`
    // and a second `host.export` read it from the run's progress key.
    let total = entries.len();
    if let Some(run_id) = progress_run_id {
        crate::import::write_progress(state, tenant, run_id, 0, total).await;
    }
    for (done, (path, data)) in entries.iter().enumerate() {
        add_entry(&mut builder, path, data)?;
        if (done + 1) % PROGRESS_EVERY == 0 && done + 1 < total {
            if let Some(run_id) = progress_run_id {
                crate::import::write_progress(state, tenant, run_id, done + 1, total).await;
            }
            tokio::task::yield_now().await;
        }
    }
    if let Some(run_id) = progress_run_id {
        crate::import::write_progress(state, tenant, run_id, total, total).await;
    }
    let encoder = builder
        .into_inner()
        .map_err(|e| AppError::Internal(format!("tar build: {e}")))?;
    encoder
        .finish()
        .map_err(|e| AppError::Internal(format!("gzip finish: {e}")))
}

/// R4: how many archive entries pass between two progress writes.
const PROGRESS_EVERY: usize = 50;

/// `manifest.json`'s `version` for archives this build writes.
pub const MANIFEST_VERSION: i64 = 2;

/// One tenant's finished archive: the `.tar.gz` bytes and the manifest's
/// `tools` list (each entry exactly a `host.tool_publish` argument object).
pub(crate) struct TenantArchive {
    pub bytes: Vec<u8>,
    pub manifest: Vec<Value>,
}

/// Rows per page when [`collect_calls`] walks the call log.
const CALLS_PAGE: i64 = 5000;

/// Every `calls` row of the tenant, newest first, read in keyset pages.
async fn collect_calls(state: &AppState, tenant_id: i64) -> Result<Vec<crate::calllog::CallRow>, AppError> {
    use crate::paged::{CallsList, CallsOrder, Paged};
    let filter = crate::calllog::CallFilter { since_unix: 0, until_unix: i64::MAX, ..Default::default() };
    let mut out: Vec<crate::calllog::CallRow> = Vec::new();
    let mut cursor = None;
    loop {
        let page = state.db.list_call_rows(tenant_id, filter.clone(), cursor.take(), CALLS_PAGE).await?;
        let full = page.len() as i64 == CALLS_PAGE;
        if let Some(last) = page.last() {
            cursor = Some(CallsList::cursor_at(last, CallsOrder::NewestFirst));
        }
        out.extend(page);
        if !full {
            return Ok(out);
        }
    }
}

/// Gathers this tenant's exportable data and builds its archive in memory.
/// Reads by the tenant's id only, so a job holding the `Tenant` row can run
/// it for a member (`org_export.rs`) without that member's credentials.
/// `progress_run_id` names the run whose progress key the tar writer updates;
/// `None` (an org member part) writes no progress. /// Gathers this tenant's exportable data and builds its archive in memory.
/// Reads by the tenant's id only, so a job holding the `Tenant` row can run
/// it for a member (`org_export.rs`) without that member's credentials.
/// `progress_run_id` names the run whose progress key the tar writer updates;
/// `None` (an org member part) writes no progress.
pub(crate) async fn build_tenant_archive(
    state: &AppState,
    tenant: &Tenant,
    progress_run_id: Option<&str>,
    tools_filter: Option<&[String]>,
    include_blobs: bool,
) -> Result<TenantArchive, AppError> {
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

    let calls = collect_calls(state, tenant.id).await?;
    let tenant_audit = state.db.export_table_rows(tenant.id, "tenant_audit", "id", &[]).await?;
    // `record_hash`/`prior_record_hash` stay, so the audit-export verifier
    // accepts the archive copy.
    let audit_chain = state.db.export_table_rows(tenant.id, "audit_chain", "id", &[]).await?;
    let oauth_audit = state.db.export_table_rows(tenant.id, "oauth_audit", "id", &[]).await?;
    // `hook_id` is the secret path segment of a webhook's URL.
    let triggers = state.db.export_table_rows(tenant.id, "triggers", "rowid", &["hook_id"]).await?;
    let blobs = state.db.blob_list(tenant.id, None, i64::MAX).await?;
    // R2: blob bytes only on request; a blob whose file is gone (`missing`) is
    // in the index and has no `blobs/<id>` entry.
    let mut blob_files: Vec<(String, Vec<u8>)> = Vec::new();
    if include_blobs {
        for blob in &blobs {
            let path = crate::blobs::blob_file(state.db.data_dir(), tenant.id, &blob.id);
            if let Ok(bytes) = tokio::fs::read(&path).await {
                blob_files.push((blob.id.clone(), bytes));
            }
        }
    }

    // P1 requirement 4 / AC5: each manifest entry is exactly a
    // `host.tool_publish` argument object -- see
    // `handler::tool_publish_input_schema` for the schema it validates
    // against.
    let manifest: Vec<Value> = tools
        .iter()
        .map(|t| json!({"name": t.name, "kind": t.kind, "spec": t.spec}))
        .collect();

    let bytes = build_tar_gz(state, tenant, progress_run_id, &ArchiveContents {
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
        calls: &calls,
        tenant_audit: &tenant_audit,
        audit_chain: &audit_chain,
        oauth_audit: &oauth_audit,
        triggers: &triggers,
        blobs: &blobs,
        blob_files: &blob_files,
    })
    .await?;
    Ok(TenantArchive { bytes, manifest })
}

/// `export_too_large` for an archive or bundle of `size` bytes over `limit`.
pub(crate) fn export_too_large_error(size: usize, limit: i64) -> AppError {
    AppError::Structured {
        code: EXPORT_TOO_LARGE,
        message: format!("export archive is {size} bytes, over this plan's {limit} byte limit"),
        data: json!({
            "size_bytes": size,
            "limit_bytes": limit,
        }),
    }
}

/// Writes a finished archive under this host's data dir (technical
/// considerations: "the tenant's scratch quota"), signs its download URL
/// with `tenant`'s own material (valid 24 h), and returns `{download_url,
/// size_bytes, expires_unix}`. Shared by [`build_archive`] and the org job,
/// which calls it with the owner's row.
pub(crate) async fn store_and_sign(
    state: &AppState,
    tenant: &Tenant,
    run_id: &str,
    archive_bytes: &[u8],
) -> Result<Value, AppError> {
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{}' is not in the loaded plan catalog",
            tenant.plan
        ))
    })?;
    if archive_bytes.len() as i64 > plan.export_bytes_max {
        return Err(export_too_large_error(archive_bytes.len(), plan.export_bytes_max));
    }

    let exports_dir = state.db.data_dir().join(EXPORT_DIR);
    tokio::fs::create_dir_all(&exports_dir)
        .await
        .map_err(|e| AppError::Storage(format!("create exports dir: {e}")))?;
    let archive_path = exports_dir.join(format!("{run_id}.tar.gz"));
    tokio::fs::write(&archive_path, archive_bytes)
        .await
        .map_err(|e| AppError::Storage(format!("write export archive: {e}")))?;

    // P0 requirement 2: valid 24h.
    let expires_unix = crate::state::now_unix() + EXPORT_URL_TTL_SECS;
    let download_url = signed_download_url(&state.public_url, &tenant.key_hash, run_id, expires_unix);

    Ok(json!({
        "download_url": download_url,
        "size_bytes": archive_bytes.len(),
        "expires_unix": expires_unix,
    }))
}

/// Builds one tenant's archive, stores it and returns the `runs.get` result
/// value (download URL, size, manifest).
async fn build_archive(
    state: &AppState,
    tenant: &Tenant,
    run_id: &str,
    tools_filter: Option<&[String]>,
    include_blobs: bool,
) -> Result<Value, AppError> {
    let archive = build_tenant_archive(state, tenant, Some(run_id), tools_filter, include_blobs).await?;
    let mut result = store_and_sign(state, tenant, run_id, &archive.bytes).await?;
    result["manifest"] = json!(archive.manifest);
    Ok(result)
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
    let is_export = run.tool_name == EXPORT_TOOL_NAME
        || run.tool_name == ENDUSER_EXPORT_TOOL_NAME
        || run.tool_name == ORG_EXPORT_TOOL_NAME
        || is_handle_export;
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

crate::err_code! {
    pub EXPORT_IN_PROGRESS,
    code: "export_in_progress", family: Storage, class: HostRefusal, retryable: false,
    remedy: "An export is already running for this tenant: poll host.runs.get with data.run_id for entries_done/entries_total instead of calling host.export again.",
    data_keys: &["run_id", "started_unix", "entries_done", "entries_total"],
}

crate::err_code! {
    pub EXPORT_TOO_LARGE,
    code: "export_too_large", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Reduce the tenant's data below the plan's export_bytes_max, or move to a larger plan.",
}

