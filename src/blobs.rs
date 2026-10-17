//! PRD-mcphost-blob-virus-and-type-guard: the one file-type table and the
//! upload guard built on it.
//!
//! [`BLOB_TYPES`] is the only place a file type is defined. It drives
//! [`sniff`], the docs path's binary refusal ([`sniff_magic`]), the upload
//! page's `accept=` attribute ([`accept_attr`]), the `accept` list a link
//! enforces ([`validate_accept`], [`default_accept`]) and the docs table
//! `gendocs` renders. [`check`] is the guard itself: sniff, extension vs
//! magic, active content, archive listing, optional ClamAV. [`upload`]
//! wraps it with the refusal wire shape (415/422/451/503 + `blob.rejected`)
//! and the "nothing is stored on refusal" rule.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::db::{BlobRow, BlobWrite, Tenant};
use crate::errors::AppError;
use crate::plans::Plan;
use crate::state::AppState;

/// Env var naming the clamd Unix socket (`MCPHOST_*` pattern as in docs.rs).
pub const CLAMD_SOCKET_ENV: &str = "MCPHOST_CLAMD_SOCKET";

/// Zip expansion ratio past which an archive is refused.
pub const MAX_ARCHIVE_RATIO: u64 = 100;

/// Default `blob_max_bytes` when the caller supplies none.
pub const DEFAULT_BLOB_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// One row of the type table.
#[derive(Debug, Clone, Copy)]
pub struct BlobType {
    pub mime: &'static str,
    pub extensions: &'static [&'static str],
    /// Magic prefixes, any of which matches at `offset`. Empty for text
    /// types (decided by [`sniff`]'s text heuristics) and for zip
    /// subtypes (decided by the archive listing).
    pub magic: &'static [&'static [u8]],
    pub offset: usize,
    /// Can carry script (svg, html): refused unless the link allows it.
    pub active_content: bool,
}

pub const BLOB_TYPES: &[BlobType] = &[
    BlobType { mime: "application/pdf", extensions: &["pdf"], magic: &[b"%PDF-"], offset: 0, active_content: false },
    BlobType { mime: "image/png", extensions: &["png"], magic: &[b"\x89PNG\r\n\x1a\n"], offset: 0, active_content: false },
    BlobType { mime: "image/jpeg", extensions: &["jpg", "jpeg"], magic: &[b"\xFF\xD8\xFF"], offset: 0, active_content: false },
    BlobType { mime: "image/gif", extensions: &["gif"], magic: &[b"GIF87a", b"GIF89a"], offset: 0, active_content: false },
    BlobType { mime: "image/webp", extensions: &["webp"], magic: &[b"RIFF"], offset: 0, active_content: false },
    BlobType { mime: "application/zip", extensions: &["zip"], magic: &[b"PK\x03\x04", b"PK\x05\x06"], offset: 0, active_content: false },
    BlobType { mime: "application/gzip", extensions: &["gz", "gzip"], magic: &[b"\x1f\x8b"], offset: 0, active_content: false },
    BlobType {
        mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        extensions: &["xlsx"],
        magic: &[],
        offset: 0,
        active_content: false,
    },
    BlobType {
        mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        extensions: &["docx"],
        magic: &[],
        offset: 0,
        active_content: false,
    },
    BlobType { mime: "text/csv", extensions: &["csv"], magic: &[], offset: 0, active_content: false },
    BlobType { mime: "text/tab-separated-values", extensions: &["tsv"], magic: &[], offset: 0, active_content: false },
    BlobType { mime: "application/json", extensions: &["json"], magic: &[], offset: 0, active_content: false },
    BlobType { mime: "text/plain", extensions: &["txt"], magic: &[], offset: 0, active_content: false },
    BlobType { mime: "text/markdown", extensions: &["md", "markdown"], magic: &[], offset: 0, active_content: false },
    BlobType { mime: "application/xml", extensions: &["xml"], magic: &[], offset: 0, active_content: false },
    BlobType { mime: "image/svg+xml", extensions: &["svg"], magic: &[], offset: 0, active_content: true },
    BlobType { mime: "text/html", extensions: &["html", "htm"], magic: &[], offset: 0, active_content: true },
];

const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const ZIP: &str = "application/zip";

/// Types whose bytes are text: any text sniff agrees with their extension.
fn is_text_mime(mime: &str) -> bool {
    matches!(
        mime,
        "text/csv" | "text/tab-separated-values" | "application/json" | "text/plain" | "text/markdown" | "application/xml"
    )
}

fn is_archive_mime(mime: &str) -> bool {
    matches!(mime, ZIP | XLSX | DOCX | "application/gzip")
}

pub fn lookup(mime: &str) -> Option<&'static BlobType> {
    BLOB_TYPES.iter().find(|t| t.mime == mime)
}

/// Every mime in the table, in table order.
pub fn all_mimes() -> Vec<&'static str> {
    BLOB_TYPES.iter().map(|t| t.mime).collect()
}

/// The mime a filename's extension names, if the table knows it.
pub fn mime_for_name(name: &str) -> Option<&'static str> {
    let (_, ext) = name.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    BLOB_TYPES.iter().find(|t| t.extensions.contains(&ext.as_str())).map(|t| t.mime)
}

/// Default `accept`: every non-active-content type (active ones too when the
/// link set `allow_active`).
pub fn default_accept(allow_active: bool) -> Vec<String> {
    BLOB_TYPES
        .iter()
        .filter(|t| allow_active || !t.active_content)
        .map(|t| t.mime.to_string())
        .collect()
}

/// R2: every entry of a link's `accept` must be a table mime; the error
/// lists the valid set, generated from the table.
pub fn validate_accept(accept: &[String]) -> Result<(), AppError> {
    let valid = all_mimes();
    for a in accept {
        if !valid.contains(&a.as_str()) {
            return Err(AppError::InvalidArgs(format!(
                "accept: '{a}' is not a known blob type; valid types are {}",
                valid.join(", ")
            )));
        }
    }
    Ok(())
}

/// The mimes named in a [`validate_accept`] error message.
pub fn mimes_listed_in_error(message: &str) -> Vec<String> {
    message
        .rsplit_once("valid types are ")
        .map(|(_, rest)| rest.split(", ").map(str::to_string).collect())
        .unwrap_or_default()
}

/// The `<input accept="…">` attribute value for a link: its mimes plus the
/// table's extensions, comma separated.
pub fn accept_attr(accept: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for m in accept {
        parts.push(m.clone());
        if let Some(t) = lookup(m) {
            parts.extend(t.extensions.iter().map(|e| format!(".{e}")));
        }
    }
    parts.join(",")
}

/// The mimes inside an [`accept_attr`] value.
pub fn mimes_in_accept_attr(attr: &str) -> Vec<String> {
    attr.split(',').filter(|p| !p.starts_with('.')).map(str::to_string).collect()
}

/// Magic-number match only (no text heuristics, no archive listing): the
/// docs path's "binary content is unsupported" check.
pub fn sniff_magic(content: &[u8]) -> Option<&'static BlobType> {
    let content = strip_utf8_bom(content);
    for t in BLOB_TYPES {
        for m in t.magic {
            if content.len() >= t.offset + m.len() && &content[t.offset..t.offset + m.len()] == *m {
                if t.mime == "image/webp" && content.get(8..12) != Some(b"WEBP") {
                    continue;
                }
                return Some(t);
            }
        }
    }
    None
}

fn strip_utf8_bom(b: &[u8]) -> &[u8] {
    b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b)
}

/// What a file's bytes are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sniffed {
    /// A table type (binary magic, zip subtype, or svg/html/xml/json text).
    Known(&'static str),
    /// Valid text with no more specific identity: csv/tsv/txt/md.
    Text,
    /// Neither a table type nor text (PE, ELF, …).
    Unknown,
}

fn utf16(bytes: &[u8], le: bool) -> Option<String> {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) })
        .collect();
    String::from_utf16(&units).ok()
}

fn decode_text(sample: &[u8]) -> Option<String> {
    if let Some(rest) = sample.strip_prefix(b"\xEF\xBB\xBF") {
        return std::str::from_utf8(trim_partial_utf8(rest)).ok().map(str::to_string);
    }
    if sample.starts_with(b"\xFF\xFE") || sample.starts_with(b"\xFE\xFF") {
        let le = sample[0] == 0xFF;
        return utf16(&sample[2..], le);
    }
    // BOM-less UTF-16: NULs at every odd (LE) or even (BE) index of ASCII text.
    if sample.len() >= 4 && sample.len().is_multiple_of(2) {
        let odd_nul = sample.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
        let even_nul = sample.iter().step_by(2).filter(|&&b| b == 0).count();
        let half = sample.len() / 2;
        if odd_nul == half && even_nul == 0 || even_nul == half && odd_nul == 0 {
            let le = odd_nul == half;
            return utf16(sample, le);
        }
    }
    if sample.contains(&0) {
        return None;
    }
    // Control bytes other than whitespace mean binary, not Latin-1 text.
    if sample.iter().any(|&b| (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0c)) || b == 0x7f) {
        return None;
    }
    match std::str::from_utf8(trim_partial_utf8(sample)) {
        Ok(s) => Some(s.to_string()),
        // Latin-1: every byte maps to a char.
        Err(_) => Some(sample.iter().map(|&b| b as char).collect()),
    }
}

/// A sample cut mid-codepoint is still valid UTF-8 up to the cut.
fn trim_partial_utf8(b: &[u8]) -> &[u8] {
    match std::str::from_utf8(b) {
        Ok(_) => b,
        Err(e) if e.error_len().is_none() => &b[..e.valid_up_to()],
        Err(_) => b,
    }
}

/// Full sniff: magic first, then zip subtype via the archive listing, then
/// text heuristics on the first 8 KiB.
pub fn sniff(content: &[u8]) -> Sniffed {
    if let Some(t) = sniff_magic(content) {
        if t.mime == ZIP
            && let Ok(entries) = list_zip(content)
        {
            if entries.iter().any(|e| e.name.starts_with("xl/")) {
                return Sniffed::Known(XLSX);
            }
            if entries.iter().any(|e| e.name.starts_with("word/")) {
                return Sniffed::Known(DOCX);
            }
        }
        return Sniffed::Known(t.mime);
    }
    let sample = &content[..content.len().min(8192)];
    let Some(text) = decode_text(sample) else {
        return Sniffed::Unknown;
    };
    let head = text.trim_start_matches('\u{feff}').trim_start().to_ascii_lowercase();
    if head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg")) {
        return Sniffed::Known("image/svg+xml");
    }
    if head.starts_with("<!doctype html") || head.starts_with("<html") {
        return Sniffed::Known("text/html");
    }
    if head.starts_with("<?xml") {
        return Sniffed::Known("application/xml");
    }
    if (head.starts_with('{') || head.starts_with('[')) && sample.len() == content.len() && serde_json::from_str::<Value>(&text).is_ok() {
        return Sniffed::Known("application/json");
    }
    Sniffed::Text
}

/// Does the filename's extension mime agree with what the bytes are?
fn extension_agrees(ext_mime: &str, sniffed: &Sniffed) -> bool {
    match sniffed {
        Sniffed::Known(m) => {
            *m == ext_mime
                // Text types read as a more specific text family.
                || (is_text_mime(ext_mime) && is_text_mime(m))
                // A csv/txt/md file can legitimately hold JSON/XML-looking text.
                || (is_text_mime(ext_mime) && *m == "application/json")
                // Office files share the zip container.
                || (ext_mime == ZIP && (*m == XLSX || *m == DOCX))
        }
        Sniffed::Text => is_text_mime(ext_mime) || ext_mime == "image/svg+xml" || ext_mime == "text/html",
        Sniffed::Unknown => false,
    }
}

/// One zip central-directory entry.
#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub name: String,
    pub compressed: u64,
    pub uncompressed: u64,
}

/// Lists a zip's central directory without extracting anything.
pub fn list_zip(data: &[u8]) -> Result<Vec<ZipEntry>, String> {
    let u16le = |o: usize| -> Option<usize> { data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize) };
    let u32le = |o: usize| -> Option<u64> { data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64) };
    let min = data.len().saturating_sub(22 + 65535);
    let eocd = (min..=data.len().saturating_sub(22))
        .rev()
        .find(|&i| data.get(i..i + 4) == Some(b"PK\x05\x06"))
        .ok_or("no end-of-central-directory record")?;
    let total = u16le(eocd + 10).ok_or("truncated EOCD")?;
    let mut off = u32le(eocd + 16).ok_or("truncated EOCD")? as usize;
    let mut out = Vec::with_capacity(total);
    for _ in 0..total {
        if data.get(off..off + 4) != Some(b"PK\x01\x02") {
            return Err("bad central directory entry".into());
        }
        let compressed = u32le(off + 20).ok_or("truncated entry")?;
        let uncompressed = u32le(off + 24).ok_or("truncated entry")?;
        let nlen = u16le(off + 28).ok_or("truncated entry")?;
        let xlen = u16le(off + 30).ok_or("truncated entry")?;
        let clen = u16le(off + 32).ok_or("truncated entry")?;
        let name = data.get(off + 46..off + 46 + nlen).ok_or("truncated entry name")?;
        out.push(ZipEntry { name: String::from_utf8_lossy(name).into_owned(), compressed, uncompressed });
        off += 46 + nlen + xlen + clen;
    }
    Ok(out)
}

/// Why an upload was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub reason: &'static str,
    pub detail: String,
    pub signature: Option<String>,
}

impl Rejection {
    fn new(reason: &'static str, detail: impl Into<String>) -> Self {
        Self { reason, detail: detail.into(), signature: None }
    }

    /// R6: 415 type, 422 archive, 451 malware, 503 scanner down.
    pub fn status(&self) -> u16 {
        match self.reason {
            "type_not_accepted" | "magic_mismatch" | "active_content" => 415,
            "archive_ratio" | "archive_path" | "archive_nested" | "archive_entry_type" => 422,
            "malware_found" => 451,
            "scan_unavailable" => 503,
            _ => 400,
        }
    }
}

/// A link's upload policy.
#[derive(Debug, Clone)]
pub struct Policy {
    /// `None` = links minted before this feature: the default list.
    pub accept: Option<Vec<String>>,
    pub allow_active: bool,
    pub zip_entries_accept: Option<Vec<String>>,
    pub blob_max_bytes: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self { accept: None, allow_active: false, zip_entries_accept: None, blob_max_bytes: DEFAULT_BLOB_MAX_BYTES }
    }
}

/// The `scan` object stored in the blob row.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub mode: &'static str,
    pub mime: String,
    pub entries: Option<usize>,
}

impl Scan {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"mode": self.mode, "result": "pass", "mime": self.mime});
        if let Some(n) = self.entries {
            v["entries"] = json!(n);
        }
        v
    }
}

/// The guard. `clamd` is the clamd socket path when one is configured.
pub fn check(name: &str, bytes: &[u8], policy: &Policy, clamd: Option<&Path>) -> Result<Scan, Rejection> {
    let sniffed = sniff(bytes);
    let accept = effective_accept(policy);
    let sniffed_label = match &sniffed {
        Sniffed::Known(m) => m.to_string(),
        Sniffed::Text => "text".to_string(),
        Sniffed::Unknown => "unrecognised binary".to_string(),
    };

    if let Some(ext_mime) = mime_for_name(name)
        && !extension_agrees(ext_mime, &sniffed)
    {
        return Err(Rejection::new("magic_mismatch", format!("'{name}' is named {ext_mime} but its bytes are {sniffed_label}")));
    }

    // The type the file is accepted as: the sniffed table type, or for plain
    // text the extension's text type (csv/tsv/txt/md), else text/plain.
    let effective: String = match &sniffed {
        Sniffed::Known(m) => {
            // A text file named .csv/.tsv/.md that looks like JSON/XML is the
            // type its extension says.
            match mime_for_name(name) {
                Some(e) if is_text_mime(e) && is_text_mime(m) => e.to_string(),
                _ => m.to_string(),
            }
        }
        Sniffed::Text => match mime_for_name(name) {
            Some(e) if is_text_mime(e) || lookup(e).is_some_and(|t| t.active_content) => e.to_string(),
            _ => "text/plain".to_string(),
        },
        Sniffed::Unknown => String::new(),
    };

    if let Some(t) = lookup(&effective)
        && t.active_content
        && !policy.allow_active
    {
        return Err(Rejection::new("active_content", format!("{} can carry script; the link does not allow active content", t.mime)));
    }
    if effective.is_empty() || !accept.contains(&effective) {
        return Err(Rejection::new(
            "type_not_accepted",
            format!("{} is not in this link's accept list", if effective.is_empty() { sniffed_label } else { effective }),
        ));
    }

    let mut entries = None;
    if effective == ZIP || effective == XLSX || effective == DOCX {
        entries = Some(check_archive(bytes, &effective, &accept, policy)?);
    }

    let mode = match clamd {
        Some(sock) => {
            scan_clamd(sock, bytes)?;
            "clamav"
        }
        None => "allowlist",
    };
    Ok(Scan { mode, mime: effective, entries })
}

fn check_archive(bytes: &[u8], mime: &str, accept: &[String], policy: &Policy) -> Result<usize, Rejection> {
    let entries = list_zip(bytes).map_err(|e| Rejection::new("archive_ratio", format!("unreadable archive: {e}")))?;
    let entries_accept: Vec<String> = match &policy.zip_entries_accept {
        Some(a) => a.clone(),
        None => accept.iter().filter(|a| !is_archive_mime(a)).cloned().collect(),
    };
    let mut declared: u64 = 0;
    for e in &entries {
        let n = e.name.replace('\\', "/");
        if n.starts_with('/') || n.split('/').any(|p| p == "..") {
            return Err(Rejection::new("archive_path", format!("entry '{}' escapes the archive root", e.name)));
        }
        declared = declared.saturating_add(e.uncompressed);
    }
    for e in &entries {
        let lower = e.name.to_ascii_lowercase();
        if lower.ends_with(".zip") {
            return Err(Rejection::new("archive_nested", format!("entry '{}' is an archive", e.name)));
        }
    }
    let ratio_limit = (bytes.len() as u64).saturating_mul(MAX_ARCHIVE_RATIO);
    if declared > ratio_limit || declared > policy.blob_max_bytes {
        return Err(Rejection::new(
            "archive_ratio",
            format!("declares {declared} bytes uncompressed from {} compressed (limit {}:1 and {} bytes)", bytes.len(), MAX_ARCHIVE_RATIO, policy.blob_max_bytes),
        ));
    }
    // Office containers hold XML parts by design; only plain zips are
    // checked entry by entry.
    if mime == ZIP {
        for e in entries.iter().filter(|e| !e.name.ends_with('/')) {
            let ok = mime_for_name(&e.name).is_some_and(|m| entries_accept.iter().any(|a| a == m));
            if !ok {
                return Err(Rejection::new("archive_entry_type", format!("entry '{}' is outside zip_entries_accept", e.name)));
            }
        }
    }
    Ok(entries.iter().filter(|e| !e.name.ends_with('/')).count())
}

static LAST_SCAN_LOG: AtomicU64 = AtomicU64::new(0);

fn log_scan_unavailable_once_per_minute(detail: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let last = LAST_SCAN_LOG.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= 60 && LAST_SCAN_LOG.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        tracing::warn!(target: "blobs", "clamd unavailable, failing closed: {detail}");
    }
}

/// Streams `bytes` through clamd's `INSTREAM` (64 KiB chunks, 20 s deadline).
/// `FOUND` -> `malware_found`; any socket problem -> `scan_unavailable`.
pub fn scan_clamd(socket: &Path, bytes: &[u8]) -> Result<(), Rejection> {
    let down = |e: String| {
        log_scan_unavailable_once_per_minute(&e);
        Rejection::new("scan_unavailable", format!("clamd: {e}"))
    };
    let mut s = std::os::unix::net::UnixStream::connect(socket).map_err(|e| down(e.to_string()))?;
    let deadline = Some(Duration::from_secs(20));
    s.set_read_timeout(deadline).map_err(|e| down(e.to_string()))?;
    s.set_write_timeout(deadline).map_err(|e| down(e.to_string()))?;
    s.write_all(b"zINSTREAM\0").map_err(|e| down(e.to_string()))?;
    for chunk in bytes.chunks(64 * 1024) {
        s.write_all(&(chunk.len() as u32).to_be_bytes()).map_err(|e| down(e.to_string()))?;
        s.write_all(chunk).map_err(|e| down(e.to_string()))?;
    }
    s.write_all(&0u32.to_be_bytes()).map_err(|e| down(e.to_string()))?;
    let mut reply = Vec::new();
    s.read_to_end(&mut reply).map_err(|e| down(e.to_string()))?;
    let text = String::from_utf8_lossy(&reply);
    let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if text.ends_with("FOUND") {
        let sig = text.strip_prefix("stream:").unwrap_or(text).trim().trim_end_matches("FOUND").trim().to_string();
        return Err(Rejection { reason: "malware_found", detail: format!("clamd reported {sig}"), signature: Some(sig) });
    }
    if text.ends_with("OK") {
        return Ok(());
    }
    Err(down(format!("unexpected reply '{text}'")))
}

/// R5: the clamd socket this deployment configured via
/// [`CLAMD_SOCKET_ENV`]; unset or empty = allowlist mode.
pub fn clamd_socket_from_env() -> Option<PathBuf> {
    std::env::var_os(CLAMD_SOCKET_ENV).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// R2: a `host.blob.upload_link` call's policy arguments (`accept`,
/// `allow_active`, `zip_entries_accept`, `blob_max_bytes`) as the link's
/// [`Policy`]. Every `accept` entry is checked by [`validate_accept`], so
/// an unknown mime is `args_invalid` listing the table.
pub fn link_policy_from_args(args: &Value) -> Result<Policy, AppError> {
    let mimes = |field: &str| -> Result<Option<Vec<String>>, AppError> {
        match args.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| v.as_str().map(str::to_string).ok_or_else(|| AppError::InvalidArgs(format!("{field}: every entry must be a mime string"))))
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(AppError::InvalidArgs(format!("{field}: must be an array of mime strings"))),
        }
    };
    let accept = mimes("accept")?;
    let zip_entries_accept = mimes("zip_entries_accept")?;
    for list in [&accept, &zip_entries_accept].into_iter().flatten() {
        validate_accept(list)?;
    }
    let allow_active = match args.get("allow_active") {
        None | Some(Value::Null) => false,
        Some(v) => v.as_bool().ok_or_else(|| AppError::InvalidArgs("allow_active: must be a boolean".into()))?,
    };
    let blob_max_bytes = match args.get("blob_max_bytes") {
        None | Some(Value::Null) => DEFAULT_BLOB_MAX_BYTES,
        Some(v) => v.as_u64().ok_or_else(|| AppError::InvalidArgs("blob_max_bytes: must be a non-negative integer".into()))?,
    };
    Ok(Policy { accept, allow_active, zip_entries_accept, blob_max_bytes })
}

/// The accept list a link enforces: its own, or the default for links that
/// set none (including links minted before this feature).
pub fn effective_accept(policy: &Policy) -> Vec<String> {
    policy.accept.clone().unwrap_or_else(|| default_accept(policy.allow_active))
}

/// R2: the upload page's file input, its `accept=` rendered from the same
/// list the guard enforces.
pub fn upload_page_html(policy: &Policy) -> String {
    format!(
        "<form method=\"post\" enctype=\"multipart/form-data\">\n<input type=\"file\" name=\"file\" accept=\"{}\">\n<button type=\"submit\">Upload</button>\n</form>\n",
        accept_attr(&effective_accept(policy))
    )
}

/// The `accept=` attribute value inside an [`upload_page_html`] page.
pub fn accept_attr_in_page(html: &str) -> Option<&str> {
    let rest = html.split_once("accept=\"")?.1;
    rest.split_once('"').map(|(v, _)| v)
}

/// Where `blob.rejected` lands. Production wiring is the tenant inbox; tests
/// use [`MemorySink`].
pub trait RejectionSink {
    fn blob_rejected(&self, message: Value);
}

#[derive(Default)]
pub struct MemorySink(pub Mutex<Vec<Value>>);

impl RejectionSink for MemorySink {
    fn blob_rejected(&self, message: Value) {
        self.0.lock().unwrap().push(message);
    }
}

/// One `/up/{token}` POST.
pub struct UploadRequest<'a> {
    pub link_token_id: &'a str,
    pub filename: &'a str,
    pub bytes: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct UploadResponse {
    pub status: u16,
    pub body: Value,
}

/// Runs the guard and either stores the blob under `blobs_dir` (200, with
/// `scan` in the body and the row) or refuses (R6) without writing anything.
pub fn upload(
    blobs_dir: &Path,
    req: &UploadRequest<'_>,
    policy: &Policy,
    clamd: Option<&Path>,
    sink: &dyn RejectionSink,
) -> UploadResponse {
    match check(req.filename, req.bytes, policy, clamd) {
        Err(rej) => {
            let mut msg = json!({
                "type": "blob.rejected",
                "reason": rej.reason,
                "detail": rej.detail,
                "name": req.filename,
                "bytes": req.bytes.len(),
                "link_token_id": req.link_token_id,
            });
            let mut body = json!({"error": rej.reason, "message": plain_reason(&rej), "detail": rej.detail});
            if let Some(sig) = &rej.signature {
                msg["signature"] = json!(sig);
                body["signature"] = json!(sig);
            }
            sink.blob_rejected(msg);
            UploadResponse { status: rej.status(), body }
        }
        Ok(scan) => match store(blobs_dir, req.filename, req.bytes, &scan) {
            Ok(id) => UploadResponse { status: 200, body: json!({"blob_id": id, "scan": scan.to_json()}) },
            Err(e) => UploadResponse { status: 500, body: json!({"error": "storage_error", "message": e.to_string()}) },
        },
    }
}

/// The `/up/{token}` POST entry point: [`upload`] with the scanner the
/// environment configures ([`clamd_socket_from_env`]).
pub fn upload_configured(blobs_dir: &Path, req: &UploadRequest<'_>, policy: &Policy, sink: &dyn RejectionSink) -> UploadResponse {
    upload(blobs_dir, req, policy, clamd_socket_from_env().as_deref(), sink)
}

fn plain_reason(rej: &Rejection) -> String {
    match rej.reason {
        "type_not_accepted" => "This file type is not accepted by this upload link.".into(),
        "magic_mismatch" => "The file's contents do not match its extension.".into(),
        "active_content" => "Files that can carry scripts (SVG, HTML) are not accepted by this link.".into(),
        "malware_found" => "The file was flagged by the virus scanner.".into(),
        "scan_unavailable" => "The virus scanner is unavailable; try again later.".into(),
        r if r.starts_with("archive_") => "This archive is not acceptable.".into(),
        _ => "Upload refused.".into(),
    }
}

/// Stores `bytes` as `<dir>/<sha256>` plus a `<sha256>.json` row holding
/// `scan_json` and the serving `Content-Disposition`.
pub fn store(dir: &Path, name: &str, bytes: &[u8], scan: &Scan) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let id: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(dir.join(&id), bytes)?;
    let active = lookup(&scan.mime).is_some_and(|t| t.active_content);
    let row = json!({
        "name": name,
        "bytes": bytes.len(),
        "mime": scan.mime,
        "scan_json": scan.to_json(),
        "content_disposition": if active { "attachment" } else { "inline" },
    });
    std::fs::write(dir.join(format!("{id}.json")), serde_json::to_vec(&row)?)?;
    Ok(id)
}

/// The response headers a stored blob is served with.
pub fn serve_headers(dir: &Path, id: &str) -> std::io::Result<Vec<(String, String)>> {
    let row: Value = serde_json::from_slice(&std::fs::read(dir.join(format!("{id}.json")))?)?;
    let mime = row["mime"].as_str().unwrap_or("application/octet-stream").to_string();
    let disp = row["content_disposition"].as_str().unwrap_or("attachment").to_string();
    Ok(vec![("Content-Type".into(), mime), ("Content-Disposition".into(), disp)])
}

/// Every file under `dir` (non-recursive helper for "nothing stored").
pub fn stored_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default()
}

// ---- host.blob.put / GET /blobs/{id} (PRD-mcphost-blob-put-get-signed-url) --
//
// Bytes live on disk at `<data_dir>/blobs/<tenant_id>/<id>`; metadata lives
// in the `blobs` table. A blob is shared by a signed URL minted through
// [`crate::signed_url`]. The type table is [`BLOB_TYPES`] above: one set of
// signatures for the guard, the docs path and `host.blob.put`.

const BLOB_DIR: &str = "blobs";

/// R2: explicit `mime`, else the name's extension row (when the content
/// agrees with it), else a content sniff, else `application/octet-stream`.
pub fn resolve_mime(explicit: Option<&str>, name: &str, content: &[u8]) -> String {
    if let Some(m) = explicit.map(str::trim).filter(|m| !m.is_empty()) {
        return m.to_string();
    }
    let sniffed = sniff(content);
    if let Some(ext_mime) = mime_for_name(name)
        && extension_agrees(ext_mime, &sniffed)
    {
        return ext_mime.to_string();
    }
    match sniffed {
        Sniffed::Known(m) => m.to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

/// "Now" for everything the blob store decides by clock: the wall clock plus
/// [`AppState::blob_clock_offset_secs`] (0 in production; a test advances it).
pub fn now(state: &AppState) -> i64 {
    crate::state::now_unix() + state.blob_clock_offset_secs.load(std::sync::atomic::Ordering::Relaxed)
}

/// R1: the longest per-blob ttl, 365 days.
pub const TTL_MAX_SECS: i64 = 365 * 24 * 60 * 60;

fn is_live(row: &BlobRow, now: i64) -> bool {
    row.expires_at.is_none_or(|e| e > now)
}

/// R1: `blob_expired` with the instant it lapsed, for `get`/`url` of a blob
/// whose row the prune has not removed yet.
fn blob_expired(expired_at: i64) -> AppError {
    AppError::Structured {
        code: BLOB_EXPIRED,
        message: format!("this blob expired at {expired_at}; host.blob.touch {{id, ttl_s}} revives it until the nightly prune removes it"),
        data: json!({"expired_at": expired_at}),
    }
}

/// `ttl_s` from the call: absent/null = `None`, else 1..=[`TTL_MAX_SECS`].
fn arg_ttl(args: &Value) -> Result<Option<i64>, AppError> {
    let Some(v) = args.get("ttl_s").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    match v.as_i64() {
        Some(t) if (1..=TTL_MAX_SECS).contains(&t) => Ok(Some(t)),
        _ => Err(AppError::InvalidArgs(format!(
            "'ttl_s' must be an integer between 1 and {TTL_MAX_SECS} seconds (365 days)"
        ))),
    }
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

/// `<data_dir>/blobs/<tenant_id>/<id>`: a blob's current bytes.
pub fn blob_file(data_dir: &std::path::Path, tenant_id: i64, id: &str) -> PathBuf {
    data_dir.join(BLOB_DIR).join(tenant_id.to_string()).join(id)
}

/// `<data_dir>/blobs/<tenant_id>/<id>.v<version>`: a superseded version the
/// tenant policy keeps (R2).
pub fn version_file(data_dir: &std::path::Path, tenant_id: i64, id: &str, version: i64) -> PathBuf {
    data_dir.join(BLOB_DIR).join(tenant_id.to_string()).join(format!("{id}.v{version}"))
}

fn blob_file_path(state: &AppState, tenant_id: i64, id: &str) -> PathBuf {
    blob_file(state.db.data_dir(), tenant_id, id)
}

fn url_for(state: &AppState, tenant: &Tenant, id: &str, expires_unix: i64) -> String {
    crate::signed_url::mint(&state.public_url, &tenant.key_hash, &blob_path(id), "expires", expires_unix)
}

fn blob_json(state: &AppState, tenant: &Tenant, row: &BlobRow) -> Value {
    let expires = now(state) + crate::signed_url::DEFAULT_TTL_SECS;
    json!({
        "id": row.id,
        "name": row.name,
        "bytes": row.bytes,
        "mime": row.mime,
        "sha256": row.sha256,
        "url": url_for(state, tenant, &row.id, expires),
        "expires": expires,
        "expires_at": row.expires_at,
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
        // `quota` names the refused ceiling; the handler moves it to
        // `data.ceiling` and puts the room-left object in its place.
        data: json!({"limit": limit, "used": used, "quota": ceiling}),
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
    let ttl_s = arg_ttl(args)?;
    let has_url = args.get("url").is_some_and(|v| !v.is_null());
    if has_url && args.get("content_base64").is_some_and(|v| !v.is_null()) {
        return Err(AppError::InvalidArgs(
            "pass exactly one of 'content_base64' or 'url'".to_string(),
        ));
    }
    if !has_url {
        let content = decode_content(args)?;
        return store_blob(state, tenant, &name, explicit_mime, ttl_s, content).await;
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
    store_blob(state, tenant, &name, mime.as_deref(), ttl_s, fetched.bytes).await
}

/// Seconds the host-side `url` fetch may take, DNS included.
const FETCH_TIMEOUT_SECS: u64 = 10;

struct Fetched {
    bytes: Vec<u8>,
    content_type: Option<String>,
}

fn host_not_allowed(host: &str) -> AppError {
    AppError::Structured {
        code: crate::uptime::HOST_NOT_ALLOWED,
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
#[allow(clippy::too_many_arguments)]
pub async fn store_blob(
    state: &AppState,
    tenant: &Tenant,
    name: &str,
    explicit_mime: Option<&str>,
    ttl_s: Option<i64>,
    content: Vec<u8>,
) -> Result<Value, AppError> {
    let bytes = content.len() as i64;
    let sha256 = crate::billing::sha256_hex(&content);
    let mime = resolve_mime(explicit_mime, name, &content);

    let now = now(state);
    let policy = state.db.blob_policy_get(tenant.id).await?;
    // R1/R2: the call's ttl_s, else the tenant policy's default, else never.
    let expires_at = ttl_s.or(policy.default_ttl_s).map(|t| now + t);
    let existing = state.db.blob_find_by_name(tenant.id, name.to_string()).await?;
    // A lapsed row awaiting the prune holds no quota and is replaced, never
    // repeated as a no-op.
    let existing_live = existing.as_ref().filter(|r| is_live(r, now));
    if let Some(row) = existing_live.filter(|r| r.sha256 == sha256 && r.mime == mime) {
        if expires_at.is_some() && expires_at != row.expires_at {
            state.db.blob_set_expiry(tenant.id, row.id.clone(), expires_at).await?;
            let mut row = row.clone();
            row.expires_at = expires_at;
            return Ok(blob_json(state, tenant, &row));
        }
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
    let (used_blobs, used_bytes) = state.db.blobs_usage_live(tenant.id, now).await?;
    if existing_live.is_none() && used_blobs >= plan.blobs_max {
        return Err(quota_error("quota_blobs", "blobs_max", plan.blobs_max, used_blobs));
    }
    let replaced_bytes = existing_live.map(|r| r.bytes).unwrap_or(0);
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

    // R2: a policy keeping several versions moves the bytes being replaced
    // aside before the new ones land on the same path.
    let keep_versions = policy.keep_latest_versions.filter(|k| *k >= 2);
    let mut archived: Option<(PathBuf, PathBuf)> = None;
    if let (Some(old), Some(_)) = (existing.as_ref(), keep_versions) {
        let from = blob_file_path(state, tenant.id, &old.id);
        let to = version_file(state.db.data_dir(), tenant.id, &old.id, old.version);
        if tokio::fs::rename(&from, &to).await.is_ok() {
            archived = Some((from, to));
        }
    }
    let new_id = crate::state::new_ulid();
    let id_for_file = existing.as_ref().map(|r| r.id.clone()).unwrap_or_else(|| new_id.clone());
    let path = blob_file_path(state, tenant.id, &id_for_file);
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

    let row = match state
        .db
        .blob_upsert(
            tenant.id,
            new_id,
            BlobWrite { name: name.to_string(), mime, bytes, sha256, expires_at, keep_versions },
            now,
        )
        .await
    {
        Ok(row) => row,
        Err(e) => {
            if existing.is_none() {
                let _ = tokio::fs::remove_file(&path).await;
            } else if let Some((from, to)) = archived {
                let _ = tokio::fs::rename(&to, &from).await;
            }
            return Err(e);
        }
    };
    Ok(blob_json(state, tenant, &row))
}

// ---- lookup and the python bridge ---------------------------------------

fn blob_not_found(what: &str) -> AppError {
    AppError::Structured {
        code: BLOB_NOT_FOUND,
        message: format!("no blob found for {what}"),
        data: json!({}),
    }
}

/// A blob of THIS tenant by `id`, or else by `name`. Another tenant's id is
/// indistinguishable from an id that does not exist.
async fn find_row(state: &AppState, tenant: &Tenant, id: Option<String>, name: Option<String>) -> Result<BlobRow, AppError> {
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

/// [`lookup`], refusing a blob past its expiry with `blob_expired`.
async fn lookup_live(state: &AppState, tenant: &Tenant, id: Option<String>, name: Option<String>) -> Result<BlobRow, AppError> {
    let row = find_row(state, tenant, id, name).await?;
    match row.expires_at {
        Some(e) if e <= now(state) => Err(blob_expired(e)),
        _ => Ok(row),
    }
}

async fn read_bytes(state: &AppState, row: &BlobRow) -> Result<Vec<u8>, AppError> {
    tokio::fs::read(blob_file_path(state, row.tenant_id, &row.id))
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
                None => find_row(state, tenant, None, Some(reference)).await?,
            };
            if let Some(e) = row.expires_at.filter(|e| *e <= now(state)) {
                return Err(blob_expired(e));
            }
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
            store_blob(state, tenant, &name, args.get("mime").and_then(Value::as_str), arg_ttl(args)?, data).await
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
    let row = lookup_live(state, tenant, id, name).await?;
    let mut out = blob_json(state, tenant, &row);
    match args.get("as").and_then(Value::as_str).unwrap_or("url") {
        "url" => Ok(out),
        "base64" => {
            if row.bytes > INLINE_MAX_BYTES {
                return Err(AppError::Structured {
                    code: BLOB_TOO_LARGE_INLINE,
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
                "expires_at": r.expires_at,
            })
        })
        .collect();
    Ok(json!({"blobs": blobs, "count": count, "total_bytes": total_bytes}))
}

/// R3: `host.blob.delete {id}`: the row and its bytes. Outstanding urls
/// answer 404 from then on.
pub async fn blob_delete(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let row = find_row(state, tenant, Some(id), None).await?;
    state.db.blob_delete(tenant.id, row.id.clone()).await?;
    let _ = tokio::fs::remove_file(blob_file_path(state, tenant.id, &row.id)).await;
    for v in state.db.blob_versions_take(tenant.id, row.id.clone()).await? {
        let _ = tokio::fs::remove_file(version_file(state.db.data_dir(), tenant.id, &row.id, v)).await;
    }
    Ok(json!({"deleted": true, "id": row.id, "name": row.name, "bytes": row.bytes}))
}

/// R1: `host.blob.touch {id, ttl_s}`: the blob now expires `ttl_s` seconds
/// from now (revives one the prune has not yet removed).
pub async fn blob_touch(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let id = arg_str(args, "id")?;
    let Some(ttl) = arg_ttl(args)? else {
        return Err(AppError::InvalidArgs("missing required argument 'ttl_s'".to_string()));
    };
    let row = find_row(state, tenant, Some(id), None).await?;
    let expires_at = now(state) + ttl;
    state.db.blob_set_expiry(tenant.id, row.id.clone(), Some(expires_at)).await?;
    Ok(json!({"id": row.id, "name": row.name, "expires_at": expires_at}))
}

/// R2: `host.blob.policy {default_ttl_s?, keep_latest_versions?, notify_before_s?}`:
/// merges the fields present into this tenant's policy and returns it. A
/// field sent as `null` clears it (`notify_before_s: 0` = off); a field left
/// out is unchanged.
pub async fn blob_policy(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let mut policy = state.db.blob_policy_get(tenant.id).await?;
    let int_field = |name: &str, lo: i64, hi: i64| -> Result<Option<Option<i64>>, AppError> {
        match args.get(name) {
            None => Ok(None),
            Some(Value::Null) => Ok(Some(None)),
            Some(v) => match v.as_i64() {
                Some(n) if (lo..=hi).contains(&n) => Ok(Some(Some(n))),
                _ => Err(AppError::InvalidArgs(format!("'{name}' must be an integer between {lo} and {hi}"))),
            },
        }
    };
    if let Some(v) = int_field("default_ttl_s", 1, TTL_MAX_SECS)? {
        policy.default_ttl_s = v;
    }
    if let Some(v) = int_field("keep_latest_versions", 1, 100)? {
        policy.keep_latest_versions = v;
    }
    if let Some(v) = int_field("notify_before_s", 0, TTL_MAX_SECS)? {
        policy.notify_before_s = v.unwrap_or(0);
    }
    state.db.blob_policy_put(tenant.id, policy.clone()).await?;
    Ok(json!({
        "default_ttl_s": policy.default_ttl_s,
        "keep_latest_versions": policy.keep_latest_versions,
        "notify_before_s": policy.notify_before_s,
    }))
}

/// R5: `host.blob.gc {dry_run: true (default), within_s?: 0}`. A dry run
/// lists what expires within `within_s` (0 = already expired); `dry_run:
/// false` deletes the already-expired set now, through the nightly prune's
/// own batch code.
pub async fn blob_gc(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let dry_run = match args.get("dry_run") {
        None | Some(Value::Null) => true,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| AppError::InvalidArgs("'dry_run' must be a boolean".to_string()))?,
    };
    let within_s = match args.get("within_s") {
        None | Some(Value::Null) => 0,
        Some(v) => v
            .as_i64()
            .filter(|n| (0..=TTL_MAX_SECS).contains(n))
            .ok_or_else(|| AppError::InvalidArgs(format!("'within_s' must be an integer between 0 and {TTL_MAX_SECS}")))?,
    };
    let now = now(state);
    if !dry_run {
        let (deleted, bytes_freed) = state.db.blob_purge_expired(tenant.id, now).await?;
        return Ok(json!({"deleted": deleted, "bytes_freed": bytes_freed}));
    }
    let items = state.db.blob_gc_list(tenant.id, now + within_s).await?;
    let bytes_total: i64 = items.iter().map(|i| i.bytes).sum();
    let expiring: Vec<Value> = items
        .iter()
        .map(|i| json!({"id": i.id, "name": i.name, "version": i.version, "bytes": i.bytes, "expires_at": i.expires_at}))
        .collect();
    Ok(json!({"expiring": expiring, "bytes_total": bytes_total, "count": items.len()}))
}

/// R6: one inbox message per tenant per day.
const NOTICE_GAP_SECS: i64 = 24 * 60 * 60;

/// R6: the hourly tick. For every tenant whose policy turns `notify_before_s`
/// on and who has live blobs lapsing within that lead time, puts ONE
/// `blob.expiring {ids, count, bytes, earliest_expires_at}` message in its
/// inbox -- and no second one until a day has passed, however often the tick
/// runs. Returns how many notices this tick sent.
pub async fn expiry_notice_tick(state: &AppState) -> Result<usize, AppError> {
    let now = now(state);
    let mut sent = 0;
    for (tenant_id, policy) in state.db.blob_notice_policies().await? {
        if policy.last_notice_unix > now - NOTICE_GAP_SECS {
            continue;
        }
        let items = state.db.blobs_expiring_between(tenant_id, now, now + policy.notify_before_s).await?;
        let Some(earliest) = items.first().map(|i| i.2) else {
            continue;
        };
        if !state.db.blob_notice_claim(tenant_id, now, NOTICE_GAP_SECS).await? {
            continue;
        }
        let ids: Vec<&str> = items.iter().map(|i| i.0.as_str()).collect();
        let bytes: i64 = items.iter().map(|i| i.1).sum();
        let data = json!({
            "kind": "blob.expiring",
            "ids": ids,
            "count": items.len(),
            "bytes": bytes,
            "earliest_expires_at": earliest,
        });
        let body = format!(
            "blob.expiring: {} file(s), {bytes} bytes, expire within {}s (earliest at {earliest}); \
             host.blob.gc {{dry_run: true, within_s}} lists them, host.blob.touch {{id, ttl_s}} extends one",
            items.len(),
            policy.notify_before_s
        );
        state
            .db
            .insert_host_notice(tenant_id, format!("blob-expiring-{tenant_id}"), "@host".to_string(), body, data.to_string())
            .await?;
        sent += 1;
    }
    Ok(sent)
}

/// R6: the hourly loop, run beside the nightly prune by
/// [`crate::retention::spawn_prune_scheduler`].
pub fn spawn_expiry_notice_loop(state: AppState, every: std::time::Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(every).await;
            if let Err(e) = expiry_notice_tick(&state).await {
                tracing::warn!(error = %e, "blob expiry notice tick failed");
            }
        }
    })
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
    let row = lookup_live(state, tenant, Some(id), None).await?;
    let expires = now(state) + ttl;
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
    AxPath(id): AxPath<String>,
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
        now(&state),
    ) {
        return (status, msg).into_response();
    }
    // R1: a blob past its expiry answers 410 even on a genuine, unexpired
    // signature, until the prune removes the row.
    if row.expires_at.is_some_and(|e| e <= now(&state)) {
        return (StatusCode::GONE, "blob expired").into_response();
    }
    match tokio::fs::read(blob_file_path(&state, row.tenant_id, &row.id)).await {
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

crate::err_code! {
    pub BLOB_EXPIRED,
    code: "blob_expired", family: Storage, class: AgentFault, retryable: false,
    remedy: "Revive it with host.blob.touch {id, ttl_s} before the nightly prune, or upload the blob again.",
}

crate::err_code! {
    pub BLOB_NOT_FOUND,
    code: "blob_not_found", family: Storage, class: AgentFault, retryable: false,
    remedy: "List blobs with host.blob.list and use an id or name that exists for this tenant.",
}

crate::err_code! {
    pub BLOB_TOO_LARGE_INLINE,
    code: "blob_too_large_inline", family: Args, class: AgentFault, retryable: false,
    remedy: "Fetch the blob through the link from host.blob.url instead of reading it inline.",
}

