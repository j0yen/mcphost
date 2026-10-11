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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::errors::AppError;

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
    let accept = policy.accept.clone().unwrap_or_else(|| default_accept(policy.allow_active));
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
