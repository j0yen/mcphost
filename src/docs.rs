//! Business logic for `host.docs.*` (PRD-mcphost-document-store P0
//! requirements 2-5, P1 requirement 7): a per-tenant document store (text,
//! markdown, JSON, CSV; up to `MCPHOST_DOC_MAX_BYTES` each) with a content
//! hash, extracted plain text, and metadata, under per-plan document and
//! byte quotas. Pure `AppState` + arguments in, `serde_json::Value` (or
//! [`AppError`]) out, same convention as `tenant_state.rs`/`tables.rs` --
//! `handler.rs` is the only place that touches `rmcp` wire types.
//!
//! Storage lives in `db.rs`'s `documents`/`document_blobs`/
//! `document_usage_events` (migration 0037); this module owns everything
//! `db.rs` deliberately doesn't: mime detection (by name, then content
//! sniff), size/quota enforcement, and deterministic text extraction
//! (requirement 3) -- the same "storage is a thin shape, business logic
//! lives beside the tool" split every other data-ish module in this crate
//! already follows.

use serde_json::{Map, Value, json};

use crate::db::{DocumentRow, Tenant};
use crate::enduser::EndUser;
use crate::errors::AppError;
use crate::plans::Plan;
use crate::rowpolicy;
use crate::state::AppState;
use crate::tables;

/// requirement 2: content ≤ this many bytes, default 2 MiB -- overridable
/// per deployment via `MCPHOST_DOC_MAX_BYTES` (the same "read once per
/// call, no cached global" convention every other env-tunable bound in this
/// crate uses, e.g. `bans::denials_threshold`).
fn max_doc_bytes() -> i64 {
    std::env::var("MCPHOST_DOC_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2 * 1024 * 1024)
}

/// requirement 2: the only mimes `host.docs.put` accepts, explicit or
/// inferred.
const ALLOWED_MIMES: &[&str] = &["text/plain", "text/markdown", "application/json", "text/csv"];

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn arg_str_opt(args: &Value, name: &str) -> Option<String> {
    args.get(name).and_then(Value::as_str).map(str::to_string)
}

fn arg_i64_opt(args: &Value, name: &str) -> Option<i64> {
    args.get(name).and_then(Value::as_i64)
}

fn arg_bool(args: &Value, name: &str) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(false)
}

fn plan_of<'a>(state: &'a AppState, plan_name: &str) -> Result<&'a Plan, AppError> {
    state.plans.get(plan_name).ok_or_else(|| {
        AppError::Internal(format!(
            "tenant's plan '{plan_name}' is not in the loaded plan catalog"
        ))
    })
}

fn doc_not_found(what: &str) -> AppError {
    AppError::Structured {
        code: "docs_not_found",
        message: format!("no document found for {what}"),
        data: json!({}),
    }
}

fn doc_too_large(bytes: i64, limit: i64) -> AppError {
    AppError::Structured {
        code: "docs_too_large",
        message: format!("document is {bytes} bytes, over the {limit} byte limit"),
        data: json!({"bytes": bytes, "limit_bytes": limit}),
    }
}

fn mime_unsupported(mime: &str) -> AppError {
    AppError::Structured {
        code: "docs_mime_unsupported",
        message: format!(
            "mime '{mime}' is not supported; allowed mimes are {ALLOWED_MIMES:?}"
        ),
        data: json!({"mime": mime, "allowed": ALLOWED_MIMES}),
    }
}

/// requirement 5/AC6: distinct wire codes for the two quota dimensions
/// (`quota_docs`/`quota_docs_bytes`), not one generic `docs_quota_exceeded`
/// with a `quota` field the way `tenant_state.rs`'s own quota check does --
/// the PRD's own AC6 wording pins the exact code names.
fn quota_docs(limit: i64, used: i64) -> AppError {
    AppError::Structured {
        code: "quota_docs",
        message: format!("docs_max quota exceeded (limit {limit}, at {used})"),
        data: json!({"limit": limit, "used": used}),
    }
}

fn quota_docs_bytes(limit: i64, used: i64) -> AppError {
    AppError::Structured {
        code: "quota_docs_bytes",
        message: format!("docs_bytes_max quota exceeded (limit {limit}, at {used})"),
        data: json!({"limit": limit, "used": used}),
    }
}

/// requirement 2: `content` (a string, stored verbatim as UTF-8 bytes) or
/// `content_base64` (arbitrary bytes) -- exactly one is required.
fn resolve_content(args: &Value) -> Result<Vec<u8>, AppError> {
    if let Some(s) = args.get("content").and_then(Value::as_str) {
        return Ok(s.as_bytes().to_vec());
    }
    if let Some(b64) = args.get("content_base64").and_then(Value::as_str) {
        use base64::Engine as _;
        return base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| AppError::InvalidArgs(format!("content_base64: not valid base64: {e}")));
    }
    Err(AppError::InvalidArgs(
        "missing required argument 'content' or 'content_base64'".to_string(),
    ))
}

/// requirement 2: a handful of common binary magic numbers -- AC8's
/// "unsupported mime (`image/png`) by content sniff" is refused here even
/// when no `mime` argument was given and the name's own extension didn't
/// already give it away.
fn sniff_binary_mime(content: &[u8]) -> Option<&'static str> {
    const SIGNATURES: &[(&[u8], &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xFF\xD8\xFF", "image/jpeg"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"%PDF-", "application/pdf"),
        (b"PK\x03\x04", "application/zip"),
    ];
    for (sig, mime) in SIGNATURES {
        if content.starts_with(sig) {
            return Some(mime);
        }
    }
    None
}

fn infer_mime_from_name(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        "text/markdown"
    } else if lower.ends_with(".json") {
        "application/json"
    } else if lower.ends_with(".csv") {
        "text/csv"
    } else {
        "text/plain"
    }
}

/// requirement 2: explicit `mime` (validated against [`ALLOWED_MIMES`]),
/// else a content sniff for a handful of common binary formats (refused --
/// AC8), else inferred from `name`'s extension.
fn resolve_mime(args: &Value, name: &str, content: &[u8]) -> Result<String, AppError> {
    if let Some(explicit) = args.get("mime").and_then(Value::as_str) {
        if !ALLOWED_MIMES.contains(&explicit) {
            return Err(mime_unsupported(explicit));
        }
        return Ok(explicit.to_string());
    }
    if let Some(sniffed) = sniff_binary_mime(content) {
        return Err(mime_unsupported(sniffed));
    }
    Ok(infer_mime_from_name(name).to_string())
}

/// requirement 3: JSON's own leaf-path flattening -- `{"a": {"b": 1}}`
/// becomes the one line `"a.b: 1"` (AC4). Object keys iterate in
/// `serde_json::Map`'s own (BTreeMap-backed, so sorted) order for
/// deterministic output; array entries flatten as `<path>.<index>`.
fn flatten_json(value: &Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten_json(v, &path, out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                let path = format!("{prefix}.{i}");
                flatten_json(v, &path, out);
            }
        }
        Value::String(s) => out.push(format!("{prefix}: {s}")),
        Value::Number(n) => out.push(format!("{prefix}: {n}")),
        Value::Bool(b) => out.push(format!("{prefix}: {b}")),
        Value::Null => out.push(format!("{prefix}: null")),
    }
}

/// requirement 3: CSV becomes one line per row, `col=value, ...` -- header
/// `x,y` / row `1,2` becomes the line `"x=1, y=2"` (AC4). A minimal,
/// unquoted-comma-split reader (no `csv` crate dependency): good enough for
/// this store's own extraction contract, not a general CSV parser.
fn extract_csv(text: &str) -> String {
    let mut lines = text.lines();
    let Some(header_line) = lines.next() else {
        return String::new();
    };
    let header: Vec<String> = header_line.split(',').map(|s| s.trim().to_string()).collect();
    let mut out = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let values: Vec<&str> = line.split(',').collect();
        let pairs: Vec<String> = header
            .iter()
            .zip(values.iter())
            .map(|(h, v)| format!("{h}={}", v.trim()))
            .collect();
        out.push(pairs.join(", "));
    }
    out.join("\n")
}

/// requirement 3: markdown/plain text pass through verbatim; JSON flattens
/// (`flatten_json`); CSV becomes `col=value` lines (`extract_csv`). Called
/// once, at `put` time -- the result is what's stored in `document_blobs`
/// and what `get {text: true}` returns, not recomputed per read.
fn extract_text(mime: &str, content: &[u8]) -> Result<String, AppError> {
    match mime {
        "text/plain" | "text/markdown" => Ok(String::from_utf8_lossy(content).into_owned()),
        "application/json" => {
            let parsed: Value = serde_json::from_slice(content)
                .map_err(|e| AppError::InvalidArgs(format!("content: not valid JSON: {e}")))?;
            let mut lines = Vec::new();
            flatten_json(&parsed, "", &mut lines);
            Ok(lines.join("\n"))
        }
        "text/csv" => Ok(extract_csv(&String::from_utf8_lossy(content))),
        other => Err(AppError::Internal(format!("extract_text: unhandled mime '{other}'"))),
    }
}

fn document_to_json(doc: &DocumentRow, include_deleted_flag: bool) -> Value {
    let metadata: Value = serde_json::from_str(&doc.metadata_json).unwrap_or(Value::Null);
    let mut obj = Map::new();
    obj.insert("id".to_string(), json!(doc.id));
    obj.insert("name".to_string(), json!(doc.name));
    obj.insert("version".to_string(), json!(doc.version));
    obj.insert("mime".to_string(), json!(doc.mime));
    obj.insert("bytes".to_string(), json!(doc.bytes));
    obj.insert("text_bytes".to_string(), json!(doc.text_bytes));
    obj.insert("metadata".to_string(), metadata);
    obj.insert("seq".to_string(), json!(doc.seq));
    obj.insert("created_at".to_string(), json!(doc.created_at));
    obj.insert("updated_at".to_string(), json!(doc.updated_at));
    if include_deleted_flag && doc.deleted_at.is_some() {
        obj.insert("deleted".to_string(), json!(true));
    }
    Value::Object(obj)
}

async fn lookup(state: &AppState, tenant: &Tenant, args: &Value) -> Result<DocumentRow, AppError> {
    if let Some(id) = arg_str_opt(args, "id") {
        return state
            .db
            .document_find_by_id(tenant.id, id.clone())
            .await?
            .ok_or_else(|| doc_not_found(&format!("id '{id}'")));
    }
    if let Some(name) = arg_str_opt(args, "name") {
        let doc = state
            .db
            .document_find_by_name(tenant.id, name.clone())
            .await?
            .ok_or_else(|| doc_not_found(&format!("name '{name}'")))?;
        if doc.deleted_at.is_some() {
            return Err(doc_not_found(&format!("name '{name}'")));
        }
        return Ok(doc);
    }
    Err(AppError::InvalidArgs(
        "missing required argument 'id' or 'name'".to_string(),
    ))
}

// ---- host.docs.put -------------------------------------------------------

/// requirement 2/AC1/AC2/AC3/AC8: detects mime, enforces the size cap and
/// this plan's `docs_max`/`docs_bytes_max` quotas, extracts text, and
/// writes through [`crate::db::Db::documents_put`]. Same name ⇒ new version
/// of the same id (AC2); identical content-hash ⇒ no-op, repeats the
/// current version (AC2).
pub async fn doc_put(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    if name.trim().is_empty() {
        return Err(AppError::InvalidArgs("'name' must not be empty".to_string()));
    }
    let content = resolve_content(args)?;

    // requirement 2/AC3: the size cap is checked before mime detection or
    // extraction -- a caller learns the size problem even if the content
    // wouldn't otherwise parse as its inferred mime.
    let max_bytes = max_doc_bytes();
    let bytes = content.len() as i64;
    if bytes > max_bytes {
        return Err(doc_too_large(bytes, max_bytes));
    }

    let mime = resolve_mime(args, &name, &content)?;
    let text = extract_text(&mime, &content)?;
    let text_bytes = text.len() as i64;
    let content_hash = crate::billing::sha256_hex(&content);
    let metadata = args.get("metadata").cloned().unwrap_or_else(|| json!({}));
    let metadata_json = serde_json::to_string(&metadata)
        .map_err(|e| AppError::Internal(format!("metadata serialize: {e}")))?;

    let plan = plan_of(state, &tenant.plan)?;
    let existing = state.db.document_find_by_name(tenant.id, name.clone()).await?;
    let (used_docs, used_bytes, _used_text_bytes) = state.db.documents_usage(tenant.id).await?;

    // requirement 5/AC6: a brand-new (or previously-deleted, being
    // recreated) name counts as one more live document against `docs_max`;
    // bumping an already-live document's version does not.
    let is_new_live_doc = existing.as_ref().map(|d| d.deleted_at.is_some()).unwrap_or(true);
    if is_new_live_doc && used_docs >= plan.docs_max {
        return Err(quota_docs(plan.docs_max, used_docs));
    }
    let old_bytes = existing
        .as_ref()
        .filter(|d| d.deleted_at.is_none())
        .map(|d| d.bytes)
        .unwrap_or(0);
    if used_bytes - old_bytes + bytes > plan.docs_bytes_max {
        return Err(quota_docs_bytes(plan.docs_bytes_max, used_bytes));
    }

    let new_id = crate::state::new_ulid();
    let now = crate::state::now_unix();
    let outcome = state
        .db
        .documents_put(
            tenant.id,
            new_id,
            name.clone(),
            content_hash.clone(),
            content,
            bytes,
            mime,
            metadata_json,
            text,
            text_bytes,
            now,
        )
        .await?;

    // requirement 5/AC7: an identical-content no-op writes nothing, so it
    // is not metered as a put either.
    if outcome.changed {
        state
            .db
            .document_usage_event_insert(tenant.id, "docs.put_bytes".to_string(), outcome.bytes, now)
            .await?;
        // PRD-mcphost-drift-review requirement 1/3: a real content change
        // (not AC2's identical-content no-op, which bumps no version)
        // records a `document` context version and enqueues this
        // document's affected searches for re-run. Best-effort: logged,
        // never fails an otherwise-successful put, same stance the
        // lineage-edge registration below already takes.
        if let Err(e) = crate::drift::versions::record_document_change(
            state,
            tenant.id,
            Some(tenant.namespace.clone()),
            &name,
            outcome.version,
            &content_hash,
        )
        .await
        {
            tracing::warn!(error = %e, tenant = %tenant.namespace, document = %name, "failed to record document drift version");
        }
    }

    // PRD-mcphost-lineage-blast-radius requirement 4: "a document is put
    // with a `derived_from` metadata key naming a table" -- registers
    // `table:<name> -> document:<name>`. `derived_from` may be a single
    // table name or an array of them. Best-effort, same convention as
    // `control::tool_publish`'s own lineage registration: logged, never
    // fails an otherwise-successful put.
    let derived_from: Vec<String> = match metadata.get("derived_from") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    for table in derived_from {
        if let Err(e) = crate::lineage::register_edge(
            state,
            tenant.id,
            (crate::lineage::NodeKind::Table, &table, &table),
            (crate::lineage::NodeKind::Document, &name, &name),
            "derived_from",
        )
        .await
        {
            tracing::warn!(error = %e, tenant = %tenant.namespace, document = %name, table = %table, "failed to register lineage edge for document derived_from");
        }
    }

    Ok(json!({
        "id": outcome.id,
        "version": outcome.version,
        "seq": outcome.seq,
        "bytes": outcome.bytes,
        "text_bytes": outcome.text_bytes,
    }))
}

// ---- host.docs.get --------------------------------------------------------

/// requirement 4/AC4/AC10: `{id | name, version?, text?}` -- `version`
/// defaults to the document's current one; a version older than the
/// document's `created_at` version that has since been purged (AC10) reads
/// as `docs_not_found`, same as an unknown id.
pub async fn doc_get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let doc = lookup(state, tenant, args).await?;
    let version = arg_i64_opt(args, "version").unwrap_or(doc.version);
    let (content, text) = state
        .db
        .document_blob_get(tenant.id, doc.id.clone(), version)
        .await?
        .ok_or_else(|| doc_not_found(&format!("id '{}' version {version}", doc.id)))?;

    let mut out = document_to_json(&doc, false);
    if let Value::Object(map) = &mut out {
        map.insert("version".to_string(), json!(version));
        use base64::Engine as _;
        map.insert(
            "content_base64".to_string(),
            json!(base64::engine::general_purpose::STANDARD.encode(&content)),
        );
        if arg_bool(args, "text") {
            map.insert("text".to_string(), json!(text));
        }
    }
    Ok(out)
}

// ---- host.docs.list -------------------------------------------------------

/// requirement 4/AC5: without `since`, the current live snapshot (no
/// `deleted` rows); with `since` (including `0`), every document whose own
/// `seq` is past it, live or deleted (deleted ones carry `deleted: true`).
pub async fn doc_list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let prefix = arg_str_opt(args, "prefix");
    let limit = arg_i64_opt(args, "limit").unwrap_or(100).clamp(1, 1000);

    let rows = match arg_i64_opt(args, "since") {
        Some(since) => {
            let cursor = arg_i64_opt(args, "cursor");
            state
                .db
                .documents_list_since(tenant.id, since, prefix, cursor, limit)
                .await?
        }
        None => {
            let cursor = arg_str_opt(args, "cursor");
            state.db.documents_list_live(tenant.id, prefix, cursor, limit).await?
        }
    };

    let next_cursor = rows.last().map(|d| json!(d.name));
    let documents: Vec<Value> = rows.iter().map(|d| document_to_json(d, true)).collect();
    Ok(json!({"documents": documents, "next_cursor": next_cursor}))
}

// ---- host.docs.delete -----------------------------------------------------

/// requirement 4: soft delete (bumps `seq`); idempotent -- deleting an
/// already-deleted document succeeds without bumping `seq` again.
pub async fn doc_delete(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let doc = if let Some(id) = arg_str_opt(args, "id") {
        state
            .db
            .document_find_by_id(tenant.id, id.clone())
            .await?
            .ok_or_else(|| doc_not_found(&format!("id '{id}'")))?
    } else if let Some(name) = arg_str_opt(args, "name") {
        state
            .db
            .document_find_by_name(tenant.id, name.clone())
            .await?
            .ok_or_else(|| doc_not_found(&format!("name '{name}'")))?
    } else {
        return Err(AppError::InvalidArgs(
            "missing required argument 'id' or 'name'".to_string(),
        ));
    };

    if doc.deleted_at.is_some() {
        return Ok(json!({"id": doc.id, "deleted": true, "seq": doc.seq}));
    }
    let now = crate::state::now_unix();
    let seq = state
        .db
        .document_soft_delete(tenant.id, doc.id.clone(), now)
        .await?
        .unwrap_or(doc.seq);
    Ok(json!({"id": doc.id, "deleted": true, "seq": seq}))
}

// ---- host.docs.status -----------------------------------------------------

/// requirement 4: `{documents, bytes, text_bytes, watermark, quota:
/// {documents_max, bytes_max}, index: {mode, indexed_watermark,
/// lag_seconds, pending_documents, chunks, rebuilding,
/// quota_chunks_reached}}` -- AC1's own post-put assertion; the `index`
/// block is PRD-mcphost-docs-semantic-search's own addition alongside the
/// document-store counters above it.
pub async fn doc_status(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let (documents, bytes, text_bytes) = state.db.documents_usage(tenant.id).await?;
    let watermark = state.db.documents_watermark(tenant.id).await?;
    let plan = plan_of(state, &tenant.plan)?;

    let now = crate::state::now_unix();
    state.db.doc_index_state_ensure(tenant.id, now).await?;
    let idx = state
        .db
        .doc_index_state_get(tenant.id)
        .await?
        .ok_or_else(|| AppError::Internal("doc_index_state row missing after ensure".into()))?;
    let pending_documents = state.db.documents_pending_count(tenant.id, idx.indexed_watermark).await?;
    let chunks = state.db.doc_chunks_count(tenant.id).await?;
    let lag_seconds = index_lag_seconds(state, tenant.id, idx.indexed_watermark, now).await?;
    let mode = if idx.provider == "openai-compatible" { "embeddings" } else { "lexical" };

    Ok(json!({
        "documents": documents,
        "bytes": bytes,
        "text_bytes": text_bytes,
        "watermark": watermark,
        "quota": {"documents_max": plan.docs_max, "bytes_max": plan.docs_bytes_max},
        "index": {
            "mode": mode,
            "indexed_watermark": idx.indexed_watermark,
            "lag_seconds": lag_seconds,
            "pending_documents": pending_documents,
            "chunks": chunks,
            "rebuilding": idx.rebuilding,
            "quota_chunks_reached": idx.quota_chunks_reached,
        },
    }))
}

/// `lag_seconds`' own computation, shared by `doc_status` and `doc_search`:
/// the age (seconds) of the oldest document still pending past `watermark`,
/// `0` once nothing is pending (the index is fully caught up).
async fn index_lag_seconds(
    state: &AppState,
    tenant_id: i64,
    watermark: i64,
    now: i64,
) -> Result<i64, AppError> {
    match state.db.documents_pending_oldest_updated_at(tenant_id, watermark).await? {
        Some(t) => Ok((now - t).max(0)),
        None => Ok(0),
    }
}

// ---- host.docs.search (P0 requirement 3) -----------------------------

fn search_filter_prefix(args: &Value) -> Option<String> {
    args.get("filter").and_then(|f| f.get("prefix")).and_then(Value::as_str).map(str::to_string)
}

fn search_filter_name(args: &Value) -> Option<String> {
    args.get("filter").and_then(|f| f.get("name")).and_then(Value::as_str).map(str::to_string)
}

/// requirement 1-6/AC1/AC3/AC4/AC5/AC6/AC10 (PRD-mcphost-docs-hybrid-search):
/// `mode` (`auto` default, `lexical`, `embeddings`, `hybrid`) picks the
/// ranking. `lexical` never touches the provider. `embeddings`/`hybrid`
/// without a configured provider is a validation error (requirement 4).
/// `auto` with a configured provider (and every explicit `hybrid` call)
/// runs both the FTS5 lexical query and the cosine ranking, each to
/// `3 x k` candidates (cap 60, requirement 1/technical considerations),
/// fuses them with [`crate::docs_index::rrf_fuse`] at
/// [`crate::docs_index::K_RRF`], and returns the top `k` with `ranks`
/// naming each hit's position in either source list (requirement 3). A
/// provider call that fails (or an empty response) degrades to the plain
/// lexical ranking with `index.mode: "lexical-fallback"`, never an error
/// (requirement 5) -- for every mode that would otherwise call the
/// provider, not just `auto`, so an operator's explicit `mode: "hybrid"`
/// during an outage gets the same fail-open behaviour `auto` always had.
/// Both candidate lists are tenant- and `filter`-scoped before fusion
/// (requirement 6/AC10): the lexical list by its own SQL `WHERE`, the
/// dense list by [`crate::docs_index::matches_filter`]. Also threaded
/// through a PRD-mcphost-row-policy AC5 docs policy filter, resolved once
/// per call and checked against every candidate chunk in every mode before
/// ranking/truncation.
pub async fn doc_search(
    state: &AppState,
    tenant: &Tenant,
    args: &Value,
    end_user: Option<&EndUser>,
) -> Result<Value, AppError> {
    let query = arg_str(args, "query")?;
    let k = args.get("k").and_then(Value::as_i64).unwrap_or(5).clamp(1, 20);
    let prefix = search_filter_prefix(args);
    let name = search_filter_name(args);
    let requested_mode = arg_str_opt(args, "mode").unwrap_or_else(|| "auto".to_string());

    let (results, mode, idx) =
        search_core(state, tenant.id, end_user, &query, k, &prefix, &name, &requested_mode).await?;

    let now = crate::state::now_unix();
    let lag_seconds = index_lag_seconds(state, tenant.id, idx.indexed_watermark, now).await?;
    let _ = state.db.document_usage_event_insert(tenant.id, "docs.search".to_string(), 1, now).await;
    // PRD-mcphost-drift-review requirement 2: mirrors `_mcphost_query_log`
    // in the same per-tenant-file pattern -- `drift::rerun`'s own
    // dependency lookup for a changed document scans this, never
    // `search_core`'s own re-run call (which must not re-log itself, or
    // every tick's re-run would enqueue its own re-run forever).
    log_docs_search(state, tenant.id, &query, &mode, &results).await;

    Ok(json!({
        "results": results,
        "index": {"mode": mode, "indexed_watermark": idx.indexed_watermark, "lag_seconds": lag_seconds},
    }))
}

/// PRD-mcphost-drift-review: `drift::rerun`'s own re-run of a logged
/// search -- same ranking logic [`doc_search`] itself runs, minus the
/// `_docs_search_log` write and `docs.search` usage-event metering (a
/// background re-run must not log itself as a new dependency, or meter
/// itself against the tenant's own quota; requirement 4 counts the whole
/// change as one call, charged once by `drift::rerun` itself).
pub(crate) async fn rerun_search(
    state: &AppState,
    tenant_id: i64,
    query: &str,
    mode: &str,
) -> Result<Vec<Value>, AppError> {
    let (results, _mode, _idx) = search_core(state, tenant_id, None, query, 5, &None, &None, mode).await?;
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
async fn search_core(
    state: &AppState,
    tenant_id: i64,
    end_user: Option<&EndUser>,
    query: &str,
    k: i64,
    prefix: &Option<String>,
    name: &Option<String>,
    requested_mode: &str,
) -> Result<(Vec<Value>, String, crate::db::DocIndexStateRow), AppError> {
    if !matches!(requested_mode, "auto" | "lexical" | "embeddings" | "hybrid") {
        return Err(AppError::InvalidArgs(format!(
            "mode must be one of 'auto', 'lexical', 'embeddings', 'hybrid', got '{requested_mode}'"
        )));
    }

    // requirement 5 (AC5): a docs policy filter, resolved once per call --
    // `None` for a tenant-key call (goal 3: policies bind end users only),
    // `Some` otherwise, checked against every candidate chunk before
    // ranking/truncation in both branches below.
    let access = rowpolicy::resolve_access(state, tenant_id, end_user).await?;
    let docs_filter: Option<(Vec<rowpolicy::RowPolicy>, rowpolicy::SecurityContext)> = match access {
        rowpolicy::Access::Unrestricted => None,
        rowpolicy::Access::Restricted(ctx) => {
            Some((rowpolicy::load_doc_prefix_policies(state, tenant_id).await?, ctx))
        }
    };
    let docs_filter_ref = docs_filter.as_ref().map(|(policies, ctx)| (policies.as_slice(), ctx));

    let now = crate::state::now_unix();
    state.db.doc_index_state_ensure(tenant_id, now).await?;
    let idx = state
        .db
        .doc_index_state_get(tenant_id)
        .await?
        .ok_or_else(|| AppError::Internal("doc_index_state row missing after ensure".into()))?;

    let provider_ready = idx.provider == "openai-compatible"
        && idx.endpoint.is_some()
        && idx.model.is_some()
        && idx.secret_name.is_some();

    if matches!(requested_mode, "hybrid" | "embeddings") && !provider_ready {
        return Err(AppError::InvalidArgs(format!(
            "mode '{requested_mode}' requires a configured embeddings provider -- call \
             host.docs.index_config with provider 'openai-compatible' first; this tenant has \
             no provider configured"
        )));
    }

    let want_hybrid_or_auto_configured =
        requested_mode == "hybrid" || (requested_mode == "auto" && provider_ready);
    let want_embeddings_only = requested_mode == "embeddings";

    let (results, mode) = if requested_mode == "lexical" {
        let hits = lexical_hits(state, tenant_id, query, k, prefix, name, docs_filter_ref).await?;
        (lexical_hits_to_json(&hits), "lexical".to_string())
    } else if want_embeddings_only {
        match embed_query(state, tenant_id, &idx, query).await {
            Some(qvec) => {
                let scored =
                    dense_candidates(state, tenant_id, &qvec, prefix, name, k, docs_filter_ref)
                        .await?;
                (embeddings_hits_to_json(&scored), "embeddings".to_string())
            }
            None => {
                let hits = lexical_hits(state, tenant_id, query, k, prefix, name, docs_filter_ref).await?;
                (lexical_hits_to_json(&hits), "lexical-fallback".to_string())
            }
        }
    } else if want_hybrid_or_auto_configured {
        match embed_query(state, tenant_id, &idx, query).await {
            Some(qvec) => {
                let results = hybrid_search_results(
                    state,
                    tenant_id,
                    &qvec,
                    query,
                    k,
                    prefix,
                    name,
                    docs_filter_ref,
                )
                .await?;
                (results, "hybrid".to_string())
            }
            None => {
                let hits = lexical_hits(state, tenant_id, query, k, prefix, name, docs_filter_ref).await?;
                (lexical_hits_to_json(&hits), "lexical-fallback".to_string())
            }
        }
    } else {
        let hits = lexical_hits(state, tenant_id, query, k, prefix, name, docs_filter_ref).await?;
        (lexical_hits_to_json(&hits), "lexical".to_string())
    };

    Ok((results, mode, idx))
}

/// PRD-mcphost-drift-review requirement 2: appends one row to this
/// tenant's `_docs_search_log` (in its own table-store file, the same
/// per-tenant pattern as `_mcphost_query_log`) -- `top_ids` is each
/// result's `(name, chunk_no)`, so `drift::rerun`'s dependency lookup for
/// a changed document can scan for a search whose top hits named it,
/// without needing to know that document's id. Best-effort: a logging
/// failure here must never fail the search itself, same stance
/// `tables::log_query` already takes for `host.table.query`.
async fn log_docs_search(state: &AppState, tenant_id: i64, query: &str, mode: &str, results: &[Value]) {
    let top_ids: Vec<Value> = results
        .iter()
        .map(|r| json!({"name": r["name"], "chunk_no": r["chunk_no"]}))
        .collect();
    let Ok(top_ids_json) = serde_json::to_string(&top_ids) else {
        return;
    };
    let path = tables::tenant_db_path(state, tenant_id);
    let query = query.to_string();
    let mode = mode.to_string();
    let outcome = tables::with_tenant_conn(path, state.db.cfg(), state.db.counters_handle(), move |conn| {
        conn.execute(
            &format!(
                "INSERT INTO {} (created_unix, query, mode, top_ids_json) VALUES (?1, ?2, ?3, ?4)",
                tables::DOCS_SEARCH_LOG_TABLE
            ),
            rusqlite::params![crate::state::now_unix(), query, mode, top_ids_json],
        )?;
        Ok(())
    })
    .await;
    if let Err(e) = outcome {
        tracing::warn!(error = %e, "failed to write host.docs.search log row");
    }
}

/// Embeds `query` through the tenant's configured provider -- `None` for
/// any transport failure, non-2xx status, malformed body, or empty
/// response, the single fail-open check every provider-calling mode
/// (`auto` with a provider, `embeddings`, `hybrid`) shares (requirement 5).
async fn embed_query(
    state: &AppState,
    tenant_id: i64,
    idx: &crate::db::DocIndexStateRow,
    query: &str,
) -> Option<Vec<f32>> {
    let (endpoint, model, secret_name) =
        (idx.endpoint.as_deref()?, idx.model.as_deref()?, idx.secret_name.as_deref()?);
    match crate::docs_index::call_embeddings_provider(
        state,
        tenant_id,
        endpoint,
        model,
        secret_name,
        std::slice::from_ref(&query.to_string()),
    )
    .await
    {
        Ok(vectors) if !vectors.is_empty() => Some(vectors[0].clone()),
        _ => None,
    }
}

/// requirement 1/technical considerations: every chunk this tenant has a
/// vector for, tenant- and `filter`-scoped
/// ([`crate::docs_index::matches_filter`]), ranked by cosine against
/// `qvec` and truncated to `width` (the caller picks `k` for an
/// embeddings-only ranking or `3 x k` capped at 60 for a hybrid dense
/// candidate list). Also filtered by a PRD-mcphost-row-policy AC5
/// `docs_filter`, when present, before truncation -- the full candidate
/// set is already in memory here, so (unlike `lexical_hits`) no
/// over-fetch is needed to avoid starving the result below `width`.
#[allow(clippy::too_many_arguments)]
async fn dense_candidates(
    state: &AppState,
    tenant_id: i64,
    qvec: &[f32],
    prefix: &Option<String>,
    name: &Option<String>,
    width: i64,
    docs_filter: Option<(&[rowpolicy::RowPolicy], &rowpolicy::SecurityContext)>,
) -> Result<Vec<(f64, crate::db::ChunkVecRow)>, AppError> {
    let rows = state.db.doc_chunks_with_vectors(tenant_id).await?;
    let mut scored: Vec<(f64, crate::db::ChunkVecRow)> = rows
        .into_iter()
        .filter(|r| crate::docs_index::matches_filter(r, prefix, name))
        .filter(|r| match docs_filter {
            Some((policies, ctx)) => rowpolicy::doc_allowed(policies, &r.name, ctx),
            None => true,
        })
        .map(|r| {
            let v = crate::docs_index::decode_vector(&r.vector);
            (crate::docs_index::cosine(qvec, &v), r)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(width.max(0) as usize);
    Ok(scored)
}

/// requirement 5: `docs_filter`, when `Some`, is checked against every
/// lexical candidate *before* the `limit` truncation a caller sees --
/// over-fetches from the DB (capped at 500) so filtering out disallowed
/// chunks doesn't silently starve the final result below `limit` when
/// enough allowed candidates exist beyond the unfiltered top-`limit`
/// window.
#[allow(clippy::too_many_arguments)]
async fn lexical_hits(
    state: &AppState,
    tenant_id: i64,
    query: &str,
    limit: i64,
    prefix: &Option<String>,
    name: &Option<String>,
    docs_filter: Option<(&[rowpolicy::RowPolicy], &rowpolicy::SecurityContext)>,
) -> Result<Vec<crate::db::ChunkHit>, AppError> {
    let Some(match_expr) = crate::docs_index::sanitize_fts_query(query) else {
        return Ok(Vec::new());
    };
    let fetch_limit = if docs_filter.is_some() { (limit * 20).min(500) } else { limit };
    let hits = state
        .db
        .doc_chunks_search_lexical(tenant_id, match_expr, prefix.clone(), name.clone(), fetch_limit)
        .await?;
    let mut hits: Vec<crate::db::ChunkHit> = hits
        .into_iter()
        .filter(|h| match docs_filter {
            Some((policies, ctx)) => rowpolicy::doc_allowed(policies, &h.name, ctx),
            None => true,
        })
        .collect();
    hits.truncate(limit as usize);
    Ok(hits)
}

/// requirement 3: a plain-lexical (or lexical-fallback) hit's `ranks` --
/// its own 1-based position in the BM25-sorted list, no embeddings rank.
fn lexical_hits_to_json(hits: &[crate::db::ChunkHit]) -> Vec<Value> {
    hits.iter()
        .enumerate()
        .map(|(i, h)| {
            json!({
                "document_id": h.document_id, "name": h.name, "version": h.version,
                "chunk_no": h.chunk_no, "offset": h.offset, "text": h.text, "score": h.score,
                "ranks": {"lexical": (i as i64) + 1, "embeddings": Value::Null},
            })
        })
        .collect()
}

/// requirement 3: an embeddings-only hit's `ranks` -- the symmetric case,
/// no lexical rank.
fn embeddings_hits_to_json(scored: &[(f64, crate::db::ChunkVecRow)]) -> Vec<Value> {
    scored
        .iter()
        .enumerate()
        .map(|(i, (score, r))| {
            json!({
                "document_id": r.document_id, "name": r.name, "version": r.version,
                "chunk_no": r.chunk_no, "offset": r.offset, "text": r.text, "score": score,
                "ranks": {"lexical": Value::Null, "embeddings": (i as i64) + 1},
            })
        })
        .collect()
}

/// A chunk's fusion identity -- `(document_id, chunk_no)` is the table's
/// own primary key minus `tenant_id`/`version` (every query here is
/// already tenant-scoped, and a chunk's `chunk_no` is unique per document
/// regardless of which `version` produced it, since the indexer always
/// deletes a document's whole prior chunk set before writing its new one).
type ChunkKey = (String, i64);

/// requirement 1/2/3 (AC1/AC2/AC10): runs the lexical and dense candidate
/// lists (each to `3 x k` capped at 60), fuses them with
/// [`crate::docs_index::rrf_fuse`], and formats the top `k` fused hits
/// with both source ranks.
#[allow(clippy::too_many_arguments)]
async fn hybrid_search_results(
    state: &AppState,
    tenant_id: i64,
    qvec: &[f32],
    query: &str,
    k: i64,
    prefix: &Option<String>,
    name: &Option<String>,
    docs_filter: Option<(&[rowpolicy::RowPolicy], &rowpolicy::SecurityContext)>,
) -> Result<Vec<Value>, AppError> {
    let width = (3 * k).min(60);

    let sparse_hits = lexical_hits(state, tenant_id, query, width, prefix, name, docs_filter).await?;
    let sparse_pairs: Vec<(ChunkKey, f64)> =
        sparse_hits.iter().map(|h| ((h.document_id.clone(), h.chunk_no), h.score)).collect();

    let dense_scored =
        dense_candidates(state, tenant_id, qvec, prefix, name, width, docs_filter).await?;
    let dense_pairs: Vec<(ChunkKey, f64)> =
        dense_scored.iter().map(|(s, r)| ((r.document_id.clone(), r.chunk_no), *s)).collect();

    let sparse_rank: std::collections::HashMap<ChunkKey, i64> =
        sparse_pairs.iter().enumerate().map(|(i, (id, _))| (id.clone(), i as i64 + 1)).collect();
    let dense_rank: std::collections::HashMap<ChunkKey, i64> =
        dense_pairs.iter().enumerate().map(|(i, (id, _))| (id.clone(), i as i64 + 1)).collect();

    let sparse_map: std::collections::HashMap<ChunkKey, &crate::db::ChunkHit> =
        sparse_hits.iter().map(|h| ((h.document_id.clone(), h.chunk_no), h)).collect();
    let dense_map: std::collections::HashMap<ChunkKey, &crate::db::ChunkVecRow> =
        dense_scored.iter().map(|(_, r)| ((r.document_id.clone(), r.chunk_no), r)).collect();

    let fused = crate::docs_index::rrf_fuse(&dense_pairs, &sparse_pairs, crate::docs_index::K_RRF);

    let mut out = Vec::with_capacity(k as usize);
    for (id, score) in fused.into_iter().take(k as usize) {
        let ranks = json!({
            "lexical": sparse_rank.get(&id).copied(),
            "embeddings": dense_rank.get(&id).copied(),
        });
        let entry = if let Some(h) = sparse_map.get(&id) {
            json!({
                "document_id": h.document_id, "name": h.name, "version": h.version,
                "chunk_no": h.chunk_no, "offset": h.offset, "text": h.text,
                "score": score, "ranks": ranks,
            })
        } else if let Some(r) = dense_map.get(&id) {
            json!({
                "document_id": r.document_id, "name": r.name, "version": r.version,
                "chunk_no": r.chunk_no, "offset": r.offset, "text": r.text,
                "score": score, "ranks": ranks,
            })
        } else {
            continue;
        };
        out.push(entry);
    }
    Ok(out)
}

// ---- host.docs.index_config (P0 requirement 4) -------------------------

/// requirement 4: `{provider: "none"|"openai-compatible", endpoint?,
/// model?, secret?, dims?}` -- switches provider config and forces a full
/// rebuild (`rebuilding: true`, watermark reset to 0).
pub async fn doc_index_config(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let provider = arg_str(args, "provider")?;
    if provider != "none" && provider != "openai-compatible" {
        return Err(AppError::InvalidArgs(format!(
            "provider must be 'none' or 'openai-compatible', got '{provider}'"
        )));
    }
    let endpoint = arg_str_opt(args, "endpoint");
    let model = arg_str_opt(args, "model");
    let secret = arg_str_opt(args, "secret");
    let dims = arg_i64_opt(args, "dims");

    if provider == "openai-compatible" {
        if endpoint.is_none() || model.is_none() || secret.is_none() {
            return Err(AppError::InvalidArgs(
                "provider 'openai-compatible' requires 'endpoint', 'model', and 'secret'".to_string(),
            ));
        }
        let secret_name = secret.clone().expect("checked above");
        if state.db.get_secret(tenant.id, secret_name.clone()).await?.is_none() {
            return Err(AppError::SecretMissing(secret_name));
        }
    }

    let now = crate::state::now_unix();
    state.db.doc_index_state_ensure(tenant.id, now).await?;
    state
        .db
        .doc_index_state_configure(tenant.id, provider.clone(), endpoint, model, secret, dims, now)
        .await?;
    Ok(json!({"provider": provider, "rebuilding": true}))
}

// ---- host.docs.reindex (P1 requirement 7) ------------------------------

/// P1 requirement 7/AC10: `{document_id?}` -- forces one document (or,
/// without `document_id`, every document) back into the pending set for
/// the indexer's next tick.
pub async fn doc_reindex(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let now = crate::state::now_unix();
    state.db.doc_index_state_ensure(tenant.id, now).await?;
    if let Some(id) = arg_str_opt(args, "document_id") {
        let doc = state
            .db
            .document_find_by_id(tenant.id, id.clone())
            .await?
            .ok_or_else(|| doc_not_found(&format!("id '{id}'")))?;
        state.db.doc_index_state_rewind_watermark(tenant.id, doc.seq - 1, now).await?;
    } else {
        state.db.doc_index_state_rewind_watermark(tenant.id, 0, now).await?;
    }
    Ok(json!({"rebuilding": true}))
}

// ---- host.docs.purge (P1) -------------------------------------------------

/// P1 requirement 7/AC10: `{id | name, older_than_versions}` -- drops every
/// stored version strictly below `current_version - older_than_versions +
/// 1`, so exactly `older_than_versions` older versions plus the current one
/// survive (a document at version 5 purged with `older_than_versions: 2`
/// keeps versions 4 and 5).
pub async fn doc_purge(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let older_than_versions = args
        .get("older_than_versions")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'older_than_versions'".to_string()))?;
    if older_than_versions < 0 {
        return Err(AppError::InvalidArgs(
            "'older_than_versions' must not be negative".to_string(),
        ));
    }
    let doc = lookup(state, tenant, args).await?;
    let keep_from_version = (doc.version - older_than_versions + 1).max(1);
    let purged = state
        .db
        .document_blobs_purge_below(tenant.id, doc.id.clone(), keep_from_version)
        .await?;
    Ok(json!({
        "id": doc.id,
        "purged_versions": purged,
        "kept_from_version": keep_from_version,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_json_matches_ac4_scenario() {
        let mut lines = Vec::new();
        flatten_json(&json!({"a": {"b": 1}}), "", &mut lines);
        assert_eq!(lines, vec!["a.b: 1".to_string()]);
    }

    #[test]
    fn extract_csv_matches_ac4_scenario() {
        assert_eq!(extract_csv("x,y\n1,2\n"), "x=1, y=2");
    }

    #[test]
    fn resolve_mime_infers_from_extension() {
        assert_eq!(infer_mime_from_name("a.md"), "text/markdown");
        assert_eq!(infer_mime_from_name("a.json"), "application/json");
        assert_eq!(infer_mime_from_name("a.csv"), "text/csv");
        assert_eq!(infer_mime_from_name("a.txt"), "text/plain");
        assert_eq!(infer_mime_from_name("a"), "text/plain");
    }

    #[test]
    fn sniff_binary_mime_detects_png() {
        let png_header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        assert_eq!(sniff_binary_mime(png_header), Some("image/png"));
        assert_eq!(sniff_binary_mime(b"just text"), None);
    }
}
