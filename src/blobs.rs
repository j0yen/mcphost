//! Business logic for `host.blob.*` (PRD-mcphost-blob-put-get-signed-url):
//! a per-tenant store for files of any type. Bytes live on disk at
//! `<data_dir>/blobs/<tenant_id>/<id>`; metadata lives in the `blobs` table
//! (migration 0083). A blob is shared by a signed URL minted through
//! [`crate::signed_url`], the same scheme `host.export` and
//! `host.table.chart` use. Same convention as `docs.rs`: pure `AppState` +
//! arguments in, `serde_json::Value` (or [`AppError`]) out.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::db::{BlobRow, BlobWrite, Tenant};
use crate::errors::AppError;
use crate::plans::Plan;
use crate::state::AppState;

const BLOB_DIR: &str = "blobs";

/// R2: one `(extension, mime, magic)` table drives both `host.blob.put`'s
/// mime resolution and `docs.rs`'s refusal of binary content. `magic` is a
/// list of `(offset, bytes)` segments that must all match; an empty list
/// means "text, decided by extension only". [`SNIFF_LEN`] bytes are enough
/// to evaluate every row.
pub struct BlobType {
    pub ext: &'static str,
    pub mime: &'static str,
    pub magic: &'static [(usize, &'static [u8])],
}

/// Bytes of a file's head the sniffer reads (the widest magic is webp's
/// `RIFF....WEBP`, 12 bytes).
pub const SNIFF_LEN: usize = 12;

pub const BLOB_TYPES: &[BlobType] = &[
    BlobType { ext: "pdf", mime: "application/pdf", magic: &[(0, b"%PDF-")] },
    BlobType { ext: "png", mime: "image/png", magic: &[(0, b"\x89PNG\r\n\x1a\n")] },
    BlobType { ext: "jpeg", mime: "image/jpeg", magic: &[(0, b"\xFF\xD8\xFF")] },
    BlobType { ext: "gif", mime: "image/gif", magic: &[(0, b"GIF87a")] },
    BlobType { ext: "gif", mime: "image/gif", magic: &[(0, b"GIF89a")] },
    BlobType { ext: "webp", mime: "image/webp", magic: &[(0, b"RIFF"), (8, b"WEBP")] },
    BlobType { ext: "zip", mime: "application/zip", magic: &[(0, b"PK\x03\x04")] },
    BlobType { ext: "gz", mime: "application/gzip", magic: &[(0, b"\x1f\x8b")] },
    BlobType {
        ext: "xlsx",
        mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        magic: &[(0, b"PK\x03\x04")],
    },
    BlobType {
        ext: "docx",
        mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        magic: &[(0, b"PK\x03\x04")],
    },
    BlobType { ext: "csv", mime: "text/csv", magic: &[] },
    BlobType { ext: "json", mime: "application/json", magic: &[] },
    BlobType { ext: "txt", mime: "text/plain", magic: &[] },
    BlobType { ext: "md", mime: "text/markdown", magic: &[] },
];

/// Extension aliases that resolve to a table row's own `ext`.
const EXT_ALIASES: &[(&str, &str)] = &[("jpg", "jpeg"), ("tgz", "gz"), ("markdown", "md")];

fn magic_matches(t: &BlobType, head: &[u8]) -> bool {
    !t.magic.is_empty()
        && t.magic
            .iter()
            .all(|(off, bytes)| head.get(*off..*off + bytes.len()).is_some_and(|s| s == *bytes))
}

/// The mime of the first binary row whose magic matches `content`'s head.
/// `docs.rs::sniff_binary_mime` is this function: one set of signatures.
pub fn sniff_content(content: &[u8]) -> Option<&'static str> {
    let head = &content[..content.len().min(SNIFF_LEN)];
    BLOB_TYPES.iter().find(|t| magic_matches(t, head)).map(|t| t.mime)
}

fn type_for_name(name: &str) -> Option<&'static BlobType> {
    let (_, ext) = name.rsplit_once('.')?;
    let lower = ext.to_ascii_lowercase();
    let canon = EXT_ALIASES.iter().find(|(a, _)| *a == lower).map(|(_, c)| *c).unwrap_or(&lower);
    BLOB_TYPES.iter().find(|t| t.ext == canon)
}

/// R2: explicit `mime`, else the name's extension row (when the content
/// agrees with that row's magic, or the row is text), else a content sniff,
/// else `application/octet-stream`.
pub fn resolve_mime(explicit: Option<&str>, name: &str, content: &[u8]) -> String {
    if let Some(m) = explicit.map(str::trim).filter(|m| !m.is_empty()) {
        return m.to_string();
    }
    let head = &content[..content.len().min(SNIFF_LEN)];
    if let Some(t) = type_for_name(name)
        && (t.magic.is_empty() || magic_matches(t, head))
    {
        return t.mime.to_string();
    }
    sniff_content(content).unwrap_or("application/octet-stream").to_string()
}

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn blob_path(id: &str) -> String {
    format!("/blobs/{id}")
}

fn file_path(state: &AppState, tenant_id: i64, id: &str) -> PathBuf {
    state.db.data_dir().join(BLOB_DIR).join(tenant_id.to_string()).join(id)
}

fn url_for(state: &AppState, tenant: &Tenant, id: &str, expires_unix: i64) -> String {
    crate::signed_url::mint(&state.public_url, &tenant.key_hash, &blob_path(id), "expires", expires_unix)
}

fn blob_json(state: &AppState, tenant: &Tenant, row: &BlobRow) -> Value {
    let expires = crate::state::now_unix() + crate::signed_url::DEFAULT_TTL_SECS;
    json!({
        "id": row.id,
        "name": row.name,
        "bytes": row.bytes,
        "mime": row.mime,
        "sha256": row.sha256,
        "url": url_for(state, tenant, &row.id, expires),
        "expires": expires,
        "version": row.version,
    })
}

fn plan_of<'a>(state: &'a AppState, plan_name: &str) -> Result<&'a Plan, AppError> {
    state.plans.get(plan_name).ok_or_else(|| {
        AppError::Internal(format!("tenant's plan '{plan_name}' is not in the loaded plan catalog"))
    })
}

/// R4: the three refusals, each carrying `{limit, used}`.
fn quota_error(code: &'static str, ceiling: &str, limit: i64, used: i64) -> AppError {
    AppError::Structured {
        code,
        message: format!("{ceiling} exceeded (limit {limit}, at {used})"),
        data: json!({"limit": limit, "used": used}),
    }
}

/// R1: `content_base64`, decoded.
fn decode_content(args: &Value) -> Result<Vec<u8>, AppError> {
    let Some(b64) = args.get("content_base64").and_then(Value::as_str) else {
        return Err(AppError::InvalidArgs(
            "missing required argument 'content_base64' (or 'url')".to_string(),
        ));
    };
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| AppError::InvalidArgs(format!("content_base64: not valid base64: {e}")))
}

fn validate_name(name: &str) -> Result<(), AppError> {
    if name.trim().is_empty() {
        return Err(AppError::InvalidArgs("'name' must not be empty".to_string()));
    }
    if name.len() > 255 || name.chars().any(char::is_control) {
        return Err(AppError::InvalidArgs(
            "'name' must be at most 255 characters with no control characters".to_string(),
        ));
    }
    Ok(())
}

// ---- host.blob.put -------------------------------------------------------

/// R1: store `content_base64` under `name`, mime resolved per R2. Same name
/// ⇒ a new version of the same id; identical sha256 ⇒ a no-op that repeats
/// the current version.
pub async fn blob_put(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let name = arg_str(args, "name")?;
    validate_name(&name)?;
    let explicit_mime = args.get("mime").and_then(Value::as_str);
    let has_url = args.get("url").is_some_and(|v| !v.is_null());
    if has_url && args.get("content_base64").is_some_and(|v| !v.is_null()) {
        return Err(AppError::InvalidArgs(
            "pass exactly one of 'content_base64' or 'url'".to_string(),
        ));
    }
    if !has_url {
        let content = decode_content(args)?;
        return store(state, tenant, &name, explicit_mime, content).await;
    }
    let url = arg_str(args, "url")?;
    let plan = plan_of(state, &tenant.plan)?;
    let fetched = fetch_remote(state, plan.blob_max_bytes, &url).await?;
    // A remote file whose name and bytes say nothing is typed by what the
    // server called it.
    let mime = match explicit_mime {
        Some(m) => Some(m.to_string()),
        None if resolve_mime(None, &name, &fetched.bytes) == "application/octet-stream" => fetched.content_type,
        None => None,
    };
    store(state, tenant, &name, mime.as_deref(), fetched.bytes).await
}

/// Seconds the host-side `url` fetch may take, DNS included.
const FETCH_TIMEOUT_SECS: u64 = 10;

struct Fetched {
    bytes: Vec<u8>,
    content_type: Option<String>,
}

fn host_not_allowed(host: &str) -> AppError {
    AppError::Structured {
        code: "host_not_allowed",
        message: format!("host '{host}' is not an allowed fetch target (private, loopback, link-local, internal or this host's own domain)"),
        data: json!({"field": "url", "host": host}),
    }
}

fn upstream(code: &'static str, message: String) -> AppError {
    AppError::Structured { code, message, data: json!({"field": "url"}) }
}

/// R1: the host-side `url` fetch -- 10 s, at most `limit` bytes, refused
/// for the same hosts the `http` kind refuses. Redirects are not followed
/// (a redirect could name a refused host), and a name's addresses are
/// vetted once and pinned for the connect so a DNS answer cannot change
/// between the check and the connection.
async fn fetch_remote(state: &AppState, limit: i64, url: &str) -> Result<Fetched, AppError> {
    use crate::kinds::http::{HickoryLookup, NameLookup, is_disallowed_ip, is_disallowed_literal_host, own_domain_from_url};

    let parsed = reqwest::Url::parse(url)
        .map_err(|e| AppError::InvalidArgs(format!("url: not a valid URL: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::InvalidArgs("url: only http and https are fetched".to_string()));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::InvalidArgs("url: has no host".to_string()))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    // Test builds fetch from a loopback stub, exactly as `authz`'s CIMD
    // fetch does; production keeps loopback refused.
    let allow_loopback = cfg!(feature = "test-support");
    if is_disallowed_literal_host(&host, &own_domain_from_url(&state.public_url), allow_loopback) {
        return Err(host_not_allowed(&host));
    }
    let port = parsed.port_or_known_default().unwrap_or(80);

    let fetch = async {
        let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
        if host.parse::<std::net::IpAddr>().is_err() {
            let lookup = HickoryLookup::new().map_err(|e| upstream("upstream_unreachable", format!("dns: {e}")))?;
            let ips = lookup
                .lookup(host.clone())
                .await
                .map_err(|e| upstream("upstream_unreachable", format!("dns lookup of '{host}' failed: {e}")))?;
            if ips.is_empty() || ips.iter().any(|ip| is_disallowed_ip(*ip, allow_loopback)) {
                return Err(host_not_allowed(&host));
            }
            let addrs: Vec<std::net::SocketAddr> =
                ips.into_iter().map(|ip| std::net::SocketAddr::new(ip, port)).collect();
            builder = builder.resolve_to_addrs(&host, &addrs);
        }
        let client = builder
            .build()
            .map_err(|e| AppError::Internal(format!("blob fetch client: {e}")))?;
        let mut resp = client
            .get(parsed.clone())
            .send()
            .await
            .map_err(|e| upstream("upstream_unreachable", format!("fetch failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let code = if status.is_client_error() { "upstream_client_error" } else { "upstream_error" };
            return Err(upstream(code, format!("the url answered HTTP {status}")));
        }
        if let Some(len) = resp.content_length()
            && len as i64 > limit
        {
            return Err(quota_error("blob_too_large", "blob_max_bytes", limit, len as i64));
        }
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty() && v != "application/octet-stream");
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| upstream("upstream_unreachable", format!("fetch interrupted: {e}")))?
        {
            if (bytes.len() + chunk.len()) as i64 > limit {
                return Err(quota_error(
                    "blob_too_large",
                    "blob_max_bytes",
                    limit,
                    (bytes.len() + chunk.len()) as i64,
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Fetched { bytes, content_type })
    };
    tokio::time::timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS), fetch)
        .await
        .map_err(|_| upstream("upstream_timeout", format!("the url did not finish within {FETCH_TIMEOUT_SECS}s")))?
}

/// The write path shared by `host.blob.put` and the python bridge.
pub async fn store(
    state: &AppState,
    tenant: &Tenant,
    name: &str,
    explicit_mime: Option<&str>,
    content: Vec<u8>,
) -> Result<Value, AppError> {
    let bytes = content.len() as i64;
    let sha256 = crate::billing::sha256_hex(&content);
    let mime = resolve_mime(explicit_mime, name, &content);

    let existing = state.db.blob_find_by_name(tenant.id, name.to_string()).await?;
    if let Some(row) = existing.as_ref().filter(|r| r.sha256 == sha256 && r.mime == mime) {
        return Ok(blob_json(state, tenant, row));
    }

    // R4: per-file, count and total-bytes ceilings, checked before any
    // write. `used` is the figure the ceiling is compared against: this
    // file's size for `blob_too_large`, the tenant's current holdings for
    // the other two.
    let plan = plan_of(state, &tenant.plan)?;
    if bytes > plan.blob_max_bytes {
        return Err(quota_error("blob_too_large", "blob_max_bytes", plan.blob_max_bytes, bytes));
    }
    let (used_blobs, used_bytes) = state.db.blobs_usage(tenant.id).await?;
    if existing.is_none() && used_blobs >= plan.blobs_max {
        return Err(quota_error("quota_blobs", "blobs_max", plan.blobs_max, used_blobs));
    }
    let replaced_bytes = existing.as_ref().map(|r| r.bytes).unwrap_or(0);
    if used_bytes - replaced_bytes + bytes > plan.blob_bytes_max {
        return Err(quota_error("quota_blob_bytes", "blob_bytes_max", plan.blob_bytes_max, used_bytes));
    }

    // The disk-floor guard wraps the write, like every other write path.
    if !state.disk_guard.is_ok(state.db.data_dir()) {
        return Err(AppError::disk_floor(
            state.disk_guard.free_bytes(state.db.data_dir()),
            state.disk_guard.floor_bytes(),
        ));
    }

    let new_id = crate::state::new_ulid();
    let id_for_file = existing.as_ref().map(|r| r.id.clone()).unwrap_or_else(|| new_id.clone());
    let path = file_path(state, tenant.id, &id_for_file);
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| AppError::Storage(format!("create blobs dir: {e}")))?;
    }
    let tmp = path.with_extension("part");
    tokio::fs::write(&tmp, &content)
        .await
        .map_err(|e| AppError::Storage(format!("write blob: {e}")))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| AppError::Storage(format!("commit blob: {e}")))?;

    let now = crate::state::now_unix();
    let row = match state
        .db
        .blob_upsert(
            tenant.id,
            new_id,
            BlobWrite { name: name.to_string(), mime, bytes, sha256 },
            now,
        )
        .await
    {
        Ok(row) => row,
        Err(e) => {
            if existing.is_none() {
                let _ = tokio::fs::remove_file(&path).await;
            }
            return Err(e);
        }
    };
    Ok(blob_json(state, tenant, &row))
}

// ---- lookup and the python bridge ---------------------------------------

fn blob_not_found(what: &str) -> AppError {
    AppError::Structured {
        code: "blob_not_found",
        message: format!("no blob found for {what}"),
        data: json!({}),
    }
}

/// A blob of THIS tenant by `id`, or else by `name`. Another tenant's id is
/// indistinguishable from an id that does not exist.
async fn lookup(state: &AppState, tenant: &Tenant, id: Option<String>, name: Option<String>) -> Result<BlobRow, AppError> {
    if let Some(id) = id {
        return state
            .db
            .blob_find_by_id(tenant.id, id.clone())
            .await?
            .ok_or_else(|| blob_not_found(&format!("id '{id}'")));
    }
    if let Some(name) = name {
        return state
            .db
            .blob_find_by_name(tenant.id, name.clone())
            .await?
            .ok_or_else(|| blob_not_found(&format!("name '{name}'")));
    }
    Err(AppError::InvalidArgs("missing required argument 'id' or 'name'".to_string()))
}

async fn read_bytes(state: &AppState, row: &BlobRow) -> Result<Vec<u8>, AppError> {
    tokio::fs::read(file_path(state, row.tenant_id, &row.id))
        .await
        .map_err(|e| AppError::Storage(format!("read blob: {e}")))
}

/// `mcphost.blob.get/put` (R6): `blob_get {ref}` returns the exact bytes
/// base64-framed; `blob_put {name, data_base64, mime?}` is `host.blob.put`.
pub async fn bridge(state: &AppState, tenant: &Tenant, op: &str, args: &Value) -> Result<Value, AppError> {
    match op {
        "blob_get" => {
            let reference = arg_str(args, "ref")?;
            let row = match state.db.blob_find_by_id(tenant.id, reference.clone()).await? {
                Some(row) => row,
                None => lookup(state, tenant, None, Some(reference)).await?,
            };
            let bytes = read_bytes(state, &row).await?;
            use base64::Engine as _;
            Ok(json!({
                "id": row.id,
                "name": row.name,
                "mime": row.mime,
                "bytes": row.bytes,
                "sha256": row.sha256,
                "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
        }
        "blob_put" => {
            let name = arg_str(args, "name")?;
            validate_name(&name)?;
            use base64::Engine as _;
            let data = base64::engine::general_purpose::STANDARD
                .decode(arg_str(args, "data_base64")?)
                .map_err(|e| AppError::InvalidArgs(format!("data_base64: not valid base64: {e}")))?;
            store(state, tenant, &name, args.get("mime").and_then(Value::as_str), data).await
        }
        other => Err(AppError::InvalidArgs(format!("unknown blob op '{other}'"))),
    }
}

// ---- host.blob.get / list / delete / url (R3, R8) ------------------------

/// `get` with `as: "base64"` refuses past this many bytes: a megabyte of
/// base64 in a tool result is the agent's context, not a download.
pub const INLINE_MAX_BYTES: i64 = 1024 * 1024;

/// R3: the longest `host.blob.url` ttl.
pub const URL_TTL_MAX_SECS: i64 = 7 * 24 * 60 * 60;

fn id_or_name(args: &Value) -> (Option<String>, Option<String>) {
    let get = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    (get("id"), get("name"))
}

/// R3: `host.blob.get {id | name, as: "base64" | "url" (default)}`.
pub async fn blob_get(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let (id, name) = id_or_name(args);
    let row = lookup(state, tenant, id, name).await?;
    let mut out = blob_json(state, tenant, &row);
    match args.get("as").and_then(Value::as_str).unwrap_or("url") {
        "url" => Ok(out),
        "base64" => {
            if row.bytes > INLINE_MAX_BYTES {
                return Err(AppError::Structured {
                    code: "blob_too_large_inline",
                    message: format!(
                        "blob is {} bytes, over the {INLINE_MAX_BYTES} byte inline limit; use host.blob.url and fetch the link",
                        row.bytes
                    ),
                    data: json!({"limit": INLINE_MAX_BYTES, "used": row.bytes, "use": "host.blob.url"}),
                });
            }
            let bytes = read_bytes(state, &row).await?;
            use base64::Engine as _;
            out["content_base64"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
            Ok(out)
        }
        other => Err(AppError::InvalidArgs(format!("'as' must be \"base64\" or \"url\", got '{other}'"))),
    }
}

/// R3/R8: `host.blob.list {prefix?, limit?}` -> this tenant's blobs by name,
/// plus `total_bytes` across all of them.
pub async fn blob_list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(100).clamp(1, 1000);
    let prefix = args.get("prefix").and_then(Value::as_str).map(str::to_string);
    let rows = state.db.blob_list(tenant.id, prefix, limit).await?;
    let (count, total_bytes) = state.db.blobs_usage(tenant.id).await?;
    let blobs: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "name": r.name,
                "bytes": r.bytes,
                "mime": r.mime,
                "sha256": r.sha256,
                "version": r.version,
                "created_at": r.created_at,
                "updated_at": r.updated_at,
            })
        })
        .collect();
    Ok(json!({"blobs": blobs, "count": count, "total_bytes": total_bytes}))
}

/// R3: `host.blob.delete {id}`: the row and its bytes. Outstanding urls
/// answer 404 from then on.
pub async fn blob_delete(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let row = lookup(state, tenant, Some(id), None).await?;
    state.db.blob_delete(tenant.id, row.id.clone()).await?;
    let _ = tokio::fs::remove_file(file_path(state, tenant.id, &row.id)).await;
    Ok(json!({"deleted": true, "id": row.id, "name": row.name, "bytes": row.bytes}))
}

/// R3: `host.blob.url {id, ttl_s ≤ 7 d}`: a fresh signed link.
pub async fn blob_url(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let ttl = args.get("ttl_s").and_then(Value::as_i64).unwrap_or(crate::signed_url::DEFAULT_TTL_SECS);
    if !(1..=URL_TTL_MAX_SECS).contains(&ttl) {
        return Err(AppError::InvalidArgs(format!(
            "'ttl_s' must be between 1 and {URL_TTL_MAX_SECS} seconds (7 days)"
        )));
    }
    let row = lookup(state, tenant, Some(id), None).await?;
    let expires = crate::state::now_unix() + ttl;
    Ok(json!({"id": row.id, "name": row.name, "url": url_for(state, tenant, &row.id, expires), "expires": expires}))
}

// ---- GET /blobs/{id} -----------------------------------------------------

/// Header-safe rendering of a blob name for `Content-Disposition`.
fn disposition_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' })
        .collect()
}

/// `GET /blobs/{id}?expires=<unix>&sig=<hex>`: unauthenticated -- the
/// signature itself is the whole auth story, same shape as
/// `export::download`. 403 for a missing/altered signature (including one
/// made with another tenant's key), 410 past expiry.
pub async fn download(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Ok(Some(row)) = state.db.blob_find_any_tenant(id.clone()).await else {
        return (StatusCode::NOT_FOUND, "blob not found").into_response();
    };
    let Ok(Some(tenant)) = state.db.find_tenant_by_id(row.tenant_id).await else {
        return (StatusCode::NOT_FOUND, "blob not found").into_response();
    };
    if let Err((status, msg)) = crate::signed_url::verify(
        &tenant.key_hash,
        &blob_path(&id),
        params.get("expires").map(String::as_str),
        params.get("sig").map(String::as_str),
        crate::state::now_unix(),
    ) {
        return (status, msg).into_response();
    }
    match tokio::fs::read(file_path(&state, row.tenant_id, &row.id)).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                ("content-type".to_string(), row.mime.clone()),
                (
                    "content-disposition".to_string(),
                    format!("attachment; filename=\"{}\"", disposition_filename(&row.name)),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "blob bytes missing").into_response(),
    }
}
