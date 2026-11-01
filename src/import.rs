//! PRD-mcphost-export-tables-and-import-restore: `host.import` -- the read
//! half of `host.export`. Restores a completed export of this host (by run
//! id, or by a signed `/exports/<run_id>` URL this host issued) into an
//! EMPTY tenant, as a background job on the same `runs` ledger the export
//! uses; `host.runs.get` reports `entries_done` / `entries_total` while it
//! runs. Every checkable refusal (unknown run, non-empty tenant, destination
//! plan's `export_bytes_max`) happens before the job starts, so a refused
//! import writes nothing.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use flate2::read::GzDecoder;
use serde_json::{Value, json};
use tar::Archive;

use crate::db::{StartExportRun, Tenant};
use crate::errors::AppError;
use crate::export::ArchiveEntry;
use crate::state::AppState;

pub const IMPORT_TOOL_NAME: &str = "host.import";

/// The state key `host.runs.get` reads an import's progress from.
pub(crate) fn progress_key(run_id: &str) -> String {
    format!("{}{run_id}/progress", crate::runs::RUN_KEY_PREFIX)
}

fn not_found() -> AppError {
    AppError::Structured {
        code: crate::subkeys::NOT_FOUND,
        message: "no completed host.export matches that run_id / url".to_string(),
        data: json!({}),
    }
}

/// What `manifest.json` says about an archive, version 1 (a bare array of
/// tool publish arguments) or 2 (`{version, tools, entries[], total_bytes}`).
struct ArchiveInfo {
    version: i64,
    tools: Vec<Value>,
    /// `(path, sha256 if the manifest carried one, bytes)`.
    entries: Vec<(String, Option<String>, i64)>,
}

impl ArchiveInfo {
    fn total_bytes(&self) -> i64 {
        self.entries.iter().map(|(_, _, b)| *b).sum()
    }
}

/// Streams just far enough to read the manifest (always the first entry);
/// a version-1 archive has no per-entry list, so its sizes are summed from
/// the tar headers instead.
fn read_info(path: &std::path::Path) -> Result<ArchiveInfo, AppError> {
    let corrupt = |m: String| AppError::Structured {
        code: ARCHIVE_CORRUPT,
        message: m,
        data: json!({}),
    };
    let file = std::fs::File::open(path).map_err(|e| AppError::Storage(format!("open export archive: {e}")))?;
    let mut archive = Archive::new(GzDecoder::new(file));
    let mut entries = archive.entries().map_err(|e| corrupt(format!("tar: {e}")))?;
    let mut first = entries
        .next()
        .ok_or_else(|| corrupt("archive is empty".to_string()))?
        .map_err(|e| corrupt(format!("tar: {e}")))?;
    let first_path = first.path().map_err(|e| corrupt(format!("tar path: {e}")))?.to_string_lossy().to_string();
    if first_path != ArchiveEntry::Manifest.path() {
        return Err(corrupt(format!("first entry is '{first_path}', not manifest.json")));
    }
    let mut raw = Vec::new();
    first.read_to_end(&mut raw).map_err(|e| corrupt(format!("read manifest: {e}")))?;
    drop(first);
    let manifest: Value = serde_json::from_slice(&raw).map_err(|e| corrupt(format!("manifest.json: {e}")))?;
    if let Value::Array(tools) = manifest {
        let mut listed = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| corrupt(format!("tar: {e}")))?;
            let p = entry.path().map_err(|e| corrupt(format!("tar path: {e}")))?.to_string_lossy().to_string();
            listed.push((p, None, entry.size() as i64));
        }
        return Ok(ArchiveInfo { version: 1, tools, entries: listed });
    }
    let version = manifest.get("version").and_then(Value::as_i64).unwrap_or(0);
    if version != crate::export::MANIFEST_VERSION {
        return Err(corrupt(format!("manifest version {version} is not readable by this host")));
    }
    let tools = manifest.get("tools").and_then(Value::as_array).cloned().unwrap_or_default();
    let listed = manifest
        .get("entries")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|e| {
                    Some((
                        e.get("path")?.as_str()?.to_string(),
                        e.get("sha256").and_then(Value::as_str).map(str::to_string),
                        e.get("bytes").and_then(Value::as_i64).unwrap_or(0),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ArchiveInfo { version, tools, entries: listed })
}

/// One archive entry: path and bytes.
type RawEntry = (String, Vec<u8>);

/// Every entry in tar order, fully read.
fn read_all(path: &std::path::Path) -> Result<Vec<RawEntry>, AppError> {
    let corrupt = |m: String| AppError::Structured { code: ARCHIVE_CORRUPT, message: m, data: json!({}) };
    let file = std::fs::File::open(path).map_err(|e| AppError::Storage(format!("open export archive: {e}")))?;
    let mut archive = Archive::new(GzDecoder::new(file));
    let mut out = Vec::new();
    for entry in archive.entries().map_err(|e| corrupt(format!("tar: {e}")))? {
        let mut entry = entry.map_err(|e| corrupt(format!("tar: {e}")))?;
        let p = entry.path().map_err(|e| corrupt(format!("tar path: {e}")))?.to_string_lossy().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).map_err(|e| corrupt(format!("read {p}: {e}")))?;
        out.push((p, data));
    }
    Ok(out)
}

/// `run_id` / `url` to the archive file of a completed `host.export` run on
/// this host. Anything else is `not_found`.
///
/// Authority: the run id (or the signed URL, which also proves the exporter
/// handed it out) is the capability -- AC3 imports tenant A's export into
/// tenant B by `run_id`. A run id of someone else's export therefore works
/// only while the export's own 24 h download window is open; the exporter's
/// own tenant has no such limit. A run that is not a completed `host.export`
/// (another tenant's tool run, an import, an unknown id) is `not_found`.
async fn resolve_archive(state: &AppState, tenant: &Tenant, args: &Value) -> Result<PathBuf, AppError> {
    let (run_id, signed) = if let Some(id) = args.get("run_id").and_then(Value::as_str) {
        (id.to_string(), None)
    } else if let Some(url) = args.get("url").and_then(Value::as_str) {
        let (path_part, query) = url.split_once('?').unwrap_or((url, ""));
        let id = path_part.rsplit_once("/exports/").map(|(_, id)| id).ok_or_else(not_found)?;
        let param = |name: &str| {
            query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v.to_string()))
        };
        let (Some(expires), Some(sig)) = (param("expires"), param("sig")) else {
            return Err(not_found());
        };
        let expires: i64 = expires.parse().map_err(|_| not_found())?;
        (id.to_string(), Some((expires, sig)))
    } else {
        return Err(AppError::InvalidArgs("host.import needs 'run_id' or 'url'".to_string()));
    };
    let run = state.db.find_run_by_id(run_id.clone()).await?.ok_or_else(not_found)?;
    if run.tool_name != crate::export::EXPORT_TOOL_NAME {
        return Err(not_found());
    }
    if let Some((expires, sig)) = signed {
        // Signed with the exporting tenant's own key material, exactly as
        // the download route verifies it.
        let owner = state.db.find_tenant_by_id(run.tenant_id).await?.ok_or_else(not_found)?;
        let expected = crate::signed_url::sign(&owner.key_hash, &format!("/exports/{run_id}"), expires);
        if !crate::auth::constant_time_eq(sig.as_bytes(), expected.as_bytes()) {
            return Err(not_found());
        }
        if crate::state::now_unix() > expires {
            return Err(AppError::Structured {
                code: EXPORT_LINK_EXPIRED,
                message: "the signed export URL has expired; run host.export again".to_string(),
                data: json!({"expires_unix": expires}),
            });
        }
    } else if run.tenant_id != tenant.id {
        let age = crate::state::now_unix() - run.finished_unix.unwrap_or(0);
        if age > crate::export::EXPORT_URL_TTL_SECS {
            return Err(not_found());
        }
    }
    if run.status != "done" {
        return Err(AppError::Structured {
            code: EXPORT_NOT_COMPLETE,
            message: format!("export run '{run_id}' is {}; import needs a completed export", run.status),
            data: json!({"run_id": run_id, "status": run.status}),
        });
    }
    let path = state.db.data_dir().join(crate::export::EXPORT_DIR).join(format!("{run_id}.tar.gz"));
    if !path.exists() {
        return Err(not_found());
    }
    Ok(path)
}

/// R2: the names of everything this tenant already holds, by kind.
async fn existing_contents(state: &AppState, tenant: &Tenant) -> Result<Value, AppError> {
    let tools: Vec<String> = state.db.list_tools(tenant.id).await?.into_iter().map(|t| t.name).collect();
    let state_keys: Vec<String> = state
        .db
        .state_kv_list(tenant.id, None, 100_000, String::new())
        .await?
        .into_iter()
        .map(|(k, _, _)| k)
        .filter(|k| !k.starts_with(crate::runs::RUN_KEY_PREFIX))
        .collect();
    let tables: Vec<String> = crate::tables::table_list(state, tenant, &json!({})).await?["tables"]
        .as_array()
        .map(|t| t.iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let docs: Vec<String> = state
        .db
        .documents_list_live(tenant.id, None, None, crate::paged::DocsOrder::NameAsc, 100_000)
        .await?
        .into_iter()
        .map(|d| d.name)
        .collect();
    Ok(json!({"tools": tools, "state_keys": state_keys, "tables": tables, "docs": docs}))
}

/// R7's preview: what a real run would write.
fn dry_run_preview(manifest_version: i64, entries: &BTreeMap<String, i64>, bytes: i64) -> Value {
    json!({"dry_run": true, "manifest_version": manifest_version, "entries": entries, "bytes": bytes})
}

/// PRD-mcphost-output-schema-reach-sweep-live R4: the schema of what
/// [`dry_run_preview`] emits (plus the optional `_meta.scratch`
/// `handler::mark_scratch` adds), attached to `host.import` as its
/// `own_dry_run` branch by `output_schemas::OWN_DRY_RUN`.
pub fn dry_run_output_branch() -> Value {
    json!({
        "type": "object",
        "required": ["dry_run", "manifest_version", "entries", "bytes"],
        "properties": {
            "dry_run": {"const": true},
            "manifest_version": {"type": "integer"},
            "entries": {"type": "object", "additionalProperties": {"type": "integer"}},
            "bytes": {"type": "integer"},
            "_meta": {"type": "object", "properties": {"scratch": {"type": "boolean"}}},
        },
    })
}

/// `host.import {run_id? | url?, dry_run?}`.
pub async fn import(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let path = resolve_archive(state, tenant, args).await?;
    let info_path = path.clone();
    let info = tokio::task::spawn_blocking(move || read_info(&info_path))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;

    let existing = existing_contents(state, tenant).await?;
    let occupied = ["tools", "state_keys", "tables", "docs"]
        .iter()
        .any(|k| existing[*k].as_array().is_some_and(|a| !a.is_empty()));
    if occupied {
        return Err(AppError::Structured {
            code: TENANT_NOT_EMPTY,
            message: "host.import restores into an empty tenant; this one already holds data".to_string(),
            data: existing,
        });
    }

    // R4: the destination plan's ceiling, checked against the manifest's
    // total before the first write.
    let plan = state.plans.get(&tenant.plan).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{}' is not in the loaded plan catalog", tenant.plan))
    })?;
    let total = info.total_bytes();
    if total > plan.export_bytes_max {
        return Err(crate::billing::quota_exceeded_for(
            &state.plans,
            state.billing_config.billing_mode(),
            &tenant.plan,
            "export_bytes_max",
            plan.export_bytes_max,
            total,
            None,
        ));
    }

    // R7: what a real run would write.
    if args.get("dry_run").and_then(Value::as_bool) == Some(true) {
        let mut counts: BTreeMap<String, i64> = BTreeMap::new();
        for (p, _, _) in &info.entries {
            let key = match ArchiveEntry::parse(p) {
                Some(e) => ArchiveEntry::layout_pattern(&e),
                None => "(unrecognised)".to_string(),
            };
            *counts.entry(key).or_default() += 1;
        }
        return Ok(dry_run_preview(info.version, &counts, total));
    }

    let run_id = crate::state::new_ulid();
    let args_json = serde_json::to_string(args).map_err(|e| AppError::Internal(format!("args serialize: {e}")))?;
    let outcome = state
        .db
        .start_export_run(tenant.id, run_id.clone(), IMPORT_TOOL_NAME.to_string(), plan.job_max_s, args_json)
        .await?;
    let run_id = match outcome {
        StartExportRun::AlreadyRunning(existing) => return Ok(json!({"run_id": existing, "status": "running"})),
        StartExportRun::Started(new_id) => new_id,
    };
    let (spawn_state, spawn_tenant, spawn_run) = (state.clone(), tenant.clone(), run_id.clone());
    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let outcome = restore(&spawn_state, &spawn_tenant, &spawn_run, path, info).await;
        // PRD-mcphost-subscriptions-listen R4: the restored tools exist now, not when
        // the verb returned.
        if outcome.is_ok() {
            spawn_state.listen_hub.notify(spawn_tenant.id, crate::listen::ListChange::Tools);
        }
        crate::export::finish_run(&spawn_state, &spawn_tenant, spawn_run, start, outcome).await;
    });
    Ok(json!({"run_id": run_id, "status": "running"}))
}

async fn write_progress(state: &AppState, tenant: &Tenant, run_id: &str, done: usize, total: usize) {
    let value = json!({"entries_done": done, "entries_total": total}).to_string();
    let _ = state.db.state_kv_set(tenant.id, progress_key(run_id), String::new(), value).await;
}

fn json_of(path: &str, data: &[u8]) -> Result<Value, AppError> {
    serde_json::from_slice(data).map_err(|e| AppError::Structured {
        code: ARCHIVE_CORRUPT,
        message: format!("{path}: {e}"),
        data: json!({}),
    })
}

/// The job body: verify every entry against the manifest, then restore in
/// archive order (the exporter writes a table's schema before its rows, and
/// a doc's meta before its bytes).
async fn restore(
    state: &AppState,
    tenant: &Tenant,
    run_id: &str,
    path: PathBuf,
    info: ArchiveInfo,
) -> Result<Value, AppError> {
    let entries = tokio::task::spawn_blocking(move || read_all(&path))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;

    // Verify before the first write: a tampered or truncated archive
    // restores nothing.
    let by_path: BTreeMap<&str, &Vec<u8>> = entries.iter().map(|(p, d)| (p.as_str(), d)).collect();
    for (p, sha, bytes) in &info.entries {
        let data = by_path.get(p.as_str()).ok_or_else(|| AppError::Structured {
            code: ARCHIVE_CORRUPT,
            message: format!("manifest lists '{p}' but the archive lacks it"),
            data: json!({"path": p}),
        })?;
        let bad = data.len() as i64 != *bytes && info.version >= 2
            || sha.as_deref().is_some_and(|s| s != crate::billing::sha256_hex(data));
        if bad {
            return Err(AppError::Structured {
                code: ARCHIVE_CORRUPT,
                message: format!("'{p}' does not match its manifest sha256/bytes"),
                data: json!({"path": p}),
            });
        }
    }

    let work: Vec<&RawEntry> = entries.iter().filter(|(p, _)| p != "manifest.json").collect();
    let total = work.len();
    write_progress(state, tenant, run_id, 0, total).await;

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut secrets_to_set: Vec<String> = Vec::new();
    let mut created_tables: Vec<String> = Vec::new();
    let mut doc_meta: BTreeMap<String, Value> = BTreeMap::new();

    for (done, (p, data)) in work.into_iter().enumerate() {
        match ArchiveEntry::parse(p) {
            None => unknown.push(p.clone()),
            Some(ArchiveEntry::ToolConfig { name }) => {
                let kind = info
                    .tools
                    .iter()
                    .find(|t| t["name"] == json!(name))
                    .and_then(|t| t["kind"].as_str())
                    .map(str::to_string);
                let spec = json_of(p, data)?;
                match kind {
                    None => skipped.push(json!({"name": name, "reason": "manifest names no kind for this tool"})),
                    Some(kind) => {
                        let publish = json!({"name": name, "kind": kind, "spec": spec});
                        match crate::control::tool_publish(state, tenant, &publish).await {
                            Ok(_) => *counts.entry("tools").or_default() += 1,
                            Err(e) => skipped.push(json!({"name": name, "reason": e.to_string()})),
                        }
                    }
                }
            }
            Some(ArchiveEntry::State { key }) => {
                let value = json_of(p, data)?;
                crate::tenant_state::state_set(state, tenant, &json!({"key": key, "value": value}), None).await?;
                *counts.entry("state").or_default() += 1;
            }
            Some(ArchiveEntry::TableSchema { name }) => {
                crate::tables::restore_table_schema(state, tenant, &name, &json_of(p, data)?).await?;
                created_tables.push(name);
                *counts.entry("tables").or_default() += 1;
            }
            Some(ArchiveEntry::TableRows { name }) => {
                if !created_tables.contains(&name) {
                    return Err(AppError::Structured {
                        code: ARCHIVE_CORRUPT,
                        message: format!("rows for table '{name}' precede its schema"),
                        data: json!({}),
                    });
                }
                let text = String::from_utf8_lossy(data);
                let rows = text
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(|l| json_of(p, l.as_bytes()))
                    .collect::<Result<Vec<_>, _>>()?;
                crate::tables::restore_table_rows(state, tenant, &name, &rows).await?;
                *counts.entry("rows").or_default() += rows.len();
            }
            Some(ArchiveEntry::Model { table }) => {
                for note in json_of(p, data)?.as_array().cloned().unwrap_or_default() {
                    let mut set = json!({"table": table, "key": note["key"], "value": note["value"]});
                    if let Some(col) = note["column"].as_str().filter(|c| !c.is_empty()) {
                        set["column"] = json!(col);
                    }
                    crate::tables_model::model_set(state, tenant, &set).await?;
                }
                *counts.entry("models").or_default() += 1;
            }
            Some(ArchiveEntry::DocMeta { name }) => {
                doc_meta.insert(name, json_of(p, data)?);
            }
            Some(ArchiveEntry::Doc { name }) => {
                let meta = doc_meta.remove(&name).unwrap_or(Value::Null);
                let mut put = json!({"name": name});
                match std::str::from_utf8(data) {
                    Ok(text) => put["content"] = json!(text),
                    Err(_) => {
                        use base64::Engine as _;
                        put["content_base64"] = json!(base64::engine::general_purpose::STANDARD.encode(data));
                    }
                }
                if let Some(mime) = meta["mime"].as_str() {
                    put["mime"] = json!(mime);
                }
                if !meta["metadata"].is_null() {
                    put["metadata"] = meta["metadata"].clone();
                }
                crate::docs::doc_put(state, tenant, &put).await?;
                *counts.entry("docs").or_default() += 1;
            }
            Some(ArchiveEntry::Secrets) => {
                secrets_to_set = String::from_utf8_lossy(data)
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            // Counted as done; nothing to write.
            Some(
                ArchiveEntry::Manifest
                | ArchiveEntry::ToolSource { .. }
                | ArchiveEntry::Document { .. }
                | ArchiveEntry::Runs
                | ArchiveEntry::Threads
                | ArchiveEntry::Usage,
            ) => {}
        }
        write_progress(state, tenant, run_id, done + 1, total).await;
        tokio::task::yield_now().await;
    }

    Ok(json!({
        "imported": counts,
        "skipped": skipped,
        "unrecognised_paths": unknown,
        "secrets_to_set": secrets_to_set,
        "entries_done": total,
        "entries_total": total,
    }))
}

crate::err_code! {
    pub ARCHIVE_CORRUPT,
    code: "archive_corrupt", family: Storage, class: AgentFault, retryable: false,
    remedy: "Re-run host.export and import the fresh archive; this one is damaged or not an export.",
}

crate::err_code! {
    pub EXPORT_LINK_EXPIRED,
    code: "export_link_expired", family: Storage, class: AgentFault, retryable: false,
    remedy: "Run host.export again to get a fresh signed link.",
}

crate::err_code! {
    pub EXPORT_NOT_COMPLETE,
    code: "export_not_complete", family: Storage, class: AgentFault, retryable: true,
    remedy: "Wait for the export run to complete, then import.",
}

crate::err_code! {
    pub TENANT_NOT_EMPTY,
    code: "tenant_not_empty", family: Storage, class: AgentFault, retryable: false,
    remedy: "Import into a new empty tenant; this one already holds data.",
}

