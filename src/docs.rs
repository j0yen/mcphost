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
use crate::errors::AppError;
use crate::plans::Plan;
use crate::state::AppState;

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
            name,
            content_hash,
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
/// {documents_max, bytes_max}}` -- AC1's own post-put assertion.
pub async fn doc_status(state: &AppState, tenant: &Tenant, _args: &Value) -> Result<Value, AppError> {
    let (documents, bytes, text_bytes) = state.db.documents_usage(tenant.id).await?;
    let watermark = state.db.documents_watermark(tenant.id).await?;
    let plan = plan_of(state, &tenant.plan)?;
    Ok(json!({
        "documents": documents,
        "bytes": bytes,
        "text_bytes": text_bytes,
        "watermark": watermark,
        "quota": {"documents_max": plan.docs_max, "bytes_max": plan.docs_bytes_max},
    }))
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
