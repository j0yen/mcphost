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

use crate::billing::hmac_sha256;
use crate::db::{MessageRow, RunRow, StartExportRun, Tenant, ToolRow};
use crate::errors::AppError;
use crate::state::AppState;

pub const EXPORT_TOOL_NAME: &str = "host.export";
const EXPORT_DIR: &str = "exports";
/// AC2: a signed download URL is valid for this long.
pub const EXPORT_URL_TTL_SECS: i64 = 24 * 60 * 60;

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

fn export_path(run_id: &str) -> String {
    format!("/exports/{run_id}")
}

/// HMAC-SHA256 over `"{path}:{expires_unix}"`, keyed by the exporting
/// tenant's own `key_hash` -- already stored server-side for every tenant,
/// so this needs no new secret (technical considerations).
fn sign(key_hash: &str, run_id: &str, expires_unix: i64) -> String {
    let message = format!("{}:{expires_unix}", export_path(run_id));
    hex_encode(&hmac_sha256(key_hash.as_bytes(), message.as_bytes()))
}

/// The full signed URL a completed export's run result carries. `pub` so
/// both [`build_archive`] (the real caller) and this crate's own test
/// suite (AC2's "after 24h, 410" case, which needs a validly-signed but
/// already-expired URL no real 24-hour wait could produce) can mint one.
pub fn signed_download_url(public_url: &str, key_hash: &str, run_id: &str, expires_unix: i64) -> String {
    let sig = sign(key_hash, run_id, expires_unix);
    format!("{public_url}{}?expires={expires_unix}&sig={sig}", export_path(run_id))
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
    let finished_unix = crate::state::now_unix();
    let duration_ms = start.elapsed().as_millis() as i64;

    let (status, result_ref, error_class) = match outcome {
        Ok(result_value) => {
            // Same "result lives in the tenant's state store under
            // runs/<run_id>" convention `runs::run_one_job` uses -- see
            // that function's own comment for why this reuses
            // `tenant_state::state_set` rather than writing the KV row
            // directly.
            let result_key = format!("runs/{run_id}");
            let set_args = json!({"key": result_key, "value": result_value});
            match crate::tenant_state::state_set(&state, &tenant, &set_args, None).await {
                Ok(_) => ("done".to_string(), Some(result_key), None),
                Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
            }
        }
        Err(e) => ("error".to_string(), None, Some(e.code().to_string())),
    };

    let _ = state
        .db
        .finalize_run(run_id, tenant.id, status, result_ref, error_class, finished_unix, duration_ms)
        .await;
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

/// Requirement 1: the archive layout -- `manifest.json`,
/// `tools/<name>/v1/{source,config.json}` for the current version (P0
/// requirement 1's "all kept versions once tool-versions ships" is
/// explicitly deferred: `tool_versions` doesn't exist yet), `state/<key>.json`
/// per stored key, `documents/<id>.json` per live document (PRD-mcphost-
/// document-store AC11: the tenant's documents with their current version
/// text, not every kept version -- old versions are `host.docs.purge`'s
/// concern, not the export's), `secrets.txt` (names only),
/// `runs.jsonl`/`threads.jsonl` (metadata only -- neither line ever carries
/// a stored run result or message body's full attachment, just the same
/// fields `host.runs.list`/`host.msg.inbox` already surface), and
/// `usage.json`.
#[allow(clippy::too_many_arguments)]
fn build_tar_gz(
    manifest: &[Value],
    tools: &[ToolRow],
    secret_names: &[String],
    state_rows: &[(String, String, i64)],
    documents: &[(String, String, i64, String)],
    run_rows: &[RunRow],
    messages: &[MessageRow],
    usage: &Value,
) -> Result<Vec<u8>, AppError> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = Builder::new(encoder);

    add_entry(&mut builder, "manifest.json", &to_json_bytes(&json!(manifest))?)?;

    for tool in tools {
        let base = format!("tools/{}/v1", tool.name);
        add_entry(&mut builder, &format!("{base}/config.json"), &to_json_bytes(&tool.spec)?)?;
        // AC1's "both tool sources": only kinds whose spec carries a
        // `source` string (e.g. `python`) get a `source` file -- a kind
        // with no such concept (`echo`) is fully represented by
        // `config.json` alone.
        if let Some(source) = tool.spec.get("source").and_then(Value::as_str) {
            add_entry(&mut builder, &format!("{base}/source"), source.as_bytes())?;
        }
    }

    for (key, value_json, _updated_unix) in state_rows {
        add_entry(&mut builder, &format!("state/{key}.json"), value_json.as_bytes())?;
    }

    // AC11: one file per live document, its current version's id/name/
    // version/extracted text.
    for (id, name, version, text) in documents {
        let doc_json = to_json_bytes(&json!({"id": id, "name": name, "version": version, "text": text}))?;
        add_entry(&mut builder, &format!("documents/{id}.json"), &doc_json)?;
    }

    // AC1: names only, never values -- `secret_names` already comes from
    // `list_secret_names`, which never touches `value_enc`/`nonce`.
    let secrets_txt = secret_names.join("\n");
    add_entry(&mut builder, "secrets.txt", secrets_txt.as_bytes())?;

    let runs_jsonl = run_rows
        .iter()
        .map(|r| {
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
        })
        .collect::<Vec<_>>()
        .join("\n");
    add_entry(&mut builder, "runs.jsonl", runs_jsonl.as_bytes())?;

    let threads_jsonl = messages
        .iter()
        .map(|m| {
            json!({
                "thread_id": m.thread_id,
                "seq": m.seq,
                "from": m.from_address,
                "body": m.body,
                "data": m.data,
                "created_at": m.created_at,
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    add_entry(&mut builder, "threads.jsonl", threads_jsonl.as_bytes())?;

    add_entry(&mut builder, "usage.json", &to_json_bytes(usage)?)?;

    let encoder = builder
        .into_inner()
        .map_err(|e| AppError::Internal(format!("tar build: {e}")))?;
    encoder
        .finish()
        .map_err(|e| AppError::Internal(format!("gzip finish: {e}")))
}

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
    let state_rows = state.db.state_kv_list(tenant.id, None, 100_000, String::new()).await?;
    let documents = state.db.documents_for_export(tenant.id).await?;
    let run_rows = state.db.list_runs(tenant.id, None, None, None, 1000).await?;
    let messages = state.db.msg_inbox(tenant.id, None, 1000, false).await?;
    let usage = crate::control::usage(state, tenant, &json!({})).await?;

    // P1 requirement 4 / AC5: each manifest entry is exactly a
    // `host.tool_publish` argument object -- see
    // `handler::tool_publish_input_schema` for the schema it validates
    // against.
    let manifest: Vec<Value> = tools
        .iter()
        .map(|t| json!({"name": t.name, "kind": t.kind, "spec": t.spec}))
        .collect();

    let archive_bytes = build_tar_gz(
        &manifest,
        &tools,
        &secret_names,
        &state_rows,
        &documents,
        &run_rows,
        &messages,
        &usage,
    )?;

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
    if run.tool_name != EXPORT_TOOL_NAME || run.status != "done" {
        return (StatusCode::NOT_FOUND, "export not found").into_response();
    }
    let Ok(Some(tenant)) = state.db.find_tenant_by_id(run.tenant_id).await else {
        return (StatusCode::NOT_FOUND, "export not found").into_response();
    };
    let (Some(expires_raw), Some(sig)) = (params.get("expires"), params.get("sig")) else {
        return (StatusCode::FORBIDDEN, "missing signature").into_response();
    };
    let Ok(expires_unix) = expires_raw.parse::<i64>() else {
        return (StatusCode::FORBIDDEN, "invalid signature").into_response();
    };
    let expected = sign(&tenant.key_hash, &run_id, expires_unix);
    if !constant_time_eq(sig.as_bytes(), expected.as_bytes()) {
        return (StatusCode::FORBIDDEN, "invalid signature").into_response();
    }
    // AC2: past its expiry, a validly-signed URL reads 410, not 403 -- the
    // signature was genuine, only stale.
    if crate::state::now_unix() > expires_unix {
        return (StatusCode::GONE, "export link expired").into_response();
    }
    let archive_path = state
        .db
        .data_dir()
        .join(EXPORT_DIR)
        .join(format!("{run_id}.tar.gz"));
    match tokio::fs::read(&archive_path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                ("content-type", "application/gzip"),
                ("content-disposition", "attachment; filename=\"export.tar.gz\""),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "export archive missing").into_response(),
    }
}
