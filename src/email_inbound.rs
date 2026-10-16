//! PRD-mcphost-email-attachments-as-blobs: the receive step that turns a
//! mail's MIME parts into blobs. Each part is decoded, guarded by the blob
//! type table, and written through the blob store's own write path
//! ([`crate::blobs::store_blob`]) under the receiving tenant; the
//! `email_received` inbox message then carries `attachments[]` with a
//! `blob_id` for each stored part and a `rejected_reason` for each refused
//! one. The message is written once, after every part is decided, and always
//! lands.

use serde_json::{Value, json};

use crate::blobs;
use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::AppState;

/// R4: parts decided per mail; the rest read `too_many_parts`.
pub const MAX_PARTS: usize = 20;

/// R3: why a part was not kept. One enum, rendered into the
/// `host.msg.inbox`/`host.msg.wait` descriptions by [`rejected_reason_help`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectedReason {
    TooLarge,
    QuotaExceeded,
    TypeNotAllowed,
    DecodeFailed,
    TooManyParts,
    StoreUnavailable,
}

impl RejectedReason {
    pub const ALL: &'static [RejectedReason] = &[
        Self::TooLarge,
        Self::QuotaExceeded,
        Self::TypeNotAllowed,
        Self::DecodeFailed,
        Self::TooManyParts,
        Self::StoreUnavailable,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::QuotaExceeded => "quota_exceeded",
            Self::TypeNotAllowed => "type_not_allowed",
            Self::DecodeFailed => "decode_failed",
            Self::TooManyParts => "too_many_parts",
            Self::StoreUnavailable => "store_unavailable",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Self::TooLarge => "the part is over the plan's blob_max_bytes",
            Self::QuotaExceeded => "the tenant is at blobs_max or blob_bytes_max (see data.quota)",
            Self::TypeNotAllowed => "the part's bytes are not an allowed blob type",
            Self::DecodeFailed => "the part's base64 did not decode",
            Self::TooManyParts => "the mail has more than 20 parts; the rest are not stored",
            Self::StoreUnavailable => "the blob store could not take the part (disk floor, scanner or storage down)",
        }
    }
}

/// R3: the sentence appended to the inbox/wait tool descriptions, generated
/// from [`RejectedReason::ALL`] so the set cannot drift from the enum.
pub fn rejected_reason_help() -> String {
    let list: Vec<String> =
        RejectedReason::ALL.iter().map(|r| format!("{} ({})", r.as_str(), r.meaning())).collect();
    format!(
        "An email_received message lists attachments[]: stored parts carry blob_id and url, refused parts carry \
         rejected_reason, one of {}.",
        list.join("; ")
    )
}

/// One MIME part as the provider delivered it.
#[derive(Debug, Clone)]
pub struct InboundPart {
    pub filename: String,
    pub content_type: String,
    pub content_base64: String,
    pub inline: bool,
}

/// One received mail, provider-neutral.
#[derive(Debug, Clone)]
pub struct InboundMail {
    /// The provider's id for the mail; redelivery of the same id is a no-op.
    pub provider_message_id: String,
    pub from: String,
    pub subject: String,
    pub parts: Vec<InboundPart>,
}

/// Resend (`attachments[{filename, content_type, content}]`) and Postmark
/// (`Attachments[{Name, ContentType, Content, ContentID}]`) payload parts.
pub fn parts_from_payload(payload: &Value) -> Vec<InboundPart> {
    let list = payload.get("attachments").or_else(|| payload.get("Attachments"));
    let Some(items) = list.and_then(Value::as_array) else {
        return Vec::new();
    };
    let s = |v: &Value, keys: &[&str]| -> String {
        keys.iter().find_map(|k| v.get(*k).and_then(Value::as_str)).unwrap_or("").to_string()
    };
    items
        .iter()
        .map(|v| InboundPart {
            filename: s(v, &["filename", "Name"]),
            content_type: s(v, &["content_type", "ContentType"]),
            content_base64: s(v, &["content", "Content"]),
            inline: v.get("inline").and_then(Value::as_bool).unwrap_or(false)
                || !s(v, &["content_id", "ContentID"]).is_empty(),
        })
        .collect()
}

/// Strip path separators and control characters, cap at 255 bytes; an empty
/// result becomes `part-<n>.<ext from mime>`.
pub fn sanitise_filename(raw: &str, index: usize, content_type: &str) -> String {
    let mut name: String = raw
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .to_string();
    while name.len() > 255 {
        name.pop();
    }
    if name.is_empty() {
        let ct = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        let ext = blobs::lookup(&ct).and_then(|t| t.extensions.first().copied()).unwrap_or("bin");
        name = format!("part-{index}.{ext}");
    }
    name
}

/// The key a mail's blobs are filed under: a hash of the provider id, so an
/// id with any characters still makes a safe blob name.
pub fn message_key(provider_message_id: &str) -> String {
    crate::billing::sha256_hex(provider_message_id.as_bytes())[..24].to_string()
}

fn blob_name(key: &str, index: usize, filename: &str) -> String {
    let prefix = format!("email/{key}/{index}-");
    let mut name = format!("{prefix}{filename}");
    while name.len() > 255 {
        name.pop();
    }
    name
}

fn refused(name: &str, mime: &str, bytes: i64, reason: RejectedReason) -> Value {
    json!({"name": name, "mime": mime, "bytes": bytes, "rejected_reason": reason.as_str()})
}

fn base64_len_estimate(b64: &str) -> i64 {
    (b64.len() as i64 / 4) * 3
}

/// The mime to show for a part's declared type; empty = unknown.
fn declared(content_type: &str) -> Option<String> {
    let ct = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    (!ct.is_empty()).then_some(ct)
}

/// The signed link for a stored part, minted now.
fn url_for(state: &AppState, tenant: &Tenant, blob_id: &str) -> String {
    let expires = blobs::now(state) + crate::signed_url::DEFAULT_TTL_SECS;
    crate::signed_url::mint(&state.public_url, &tenant.key_hash, &format!("/blobs/{blob_id}"), "expires", expires)
}

/// Decide one part: `Ok(entry)` is the `attachments[]` object; the bool says
/// the part was refused for quota.
async fn decide_part(
    state: &AppState,
    tenant: &Tenant,
    key: &str,
    index: usize,
    part: &InboundPart,
) -> (Value, bool) {
    let name = sanitise_filename(&part.filename, index, &part.content_type);
    let declared_mime = declared(&part.content_type);
    let shown_mime = declared_mime.clone().unwrap_or_else(|| "application/octet-stream".to_string());

    // A part this mail already stored (a redelivery) is reported as it was.
    if let Ok(Some((row, declared_stored, inline))) =
        state.db.blob_find_email_part(tenant.id, key.to_string(), index as i64).await
    {
        return (stored_entry(state, tenant, &name, &row.id, &row.mime, row.bytes, declared_stored, inline), false);
    }

    use base64::Engine as _;
    let bytes = match base64::engine::general_purpose::STANDARD.decode(part.content_base64.trim()) {
        Ok(b) => b,
        Err(_) => return (refused(&name, &shown_mime, 0, RejectedReason::DecodeFailed), false),
    };
    let len = bytes.len() as i64;
    let Some(plan) = state.plans.get(&tenant.plan) else {
        return (refused(&name, &shown_mime, len, RejectedReason::StoreUnavailable), false);
    };
    if len > plan.blob_max_bytes {
        return (refused(&name, &shown_mime, len, RejectedReason::TooLarge), false);
    }
    let policy = blobs::Policy { blob_max_bytes: plan.blob_max_bytes as u64, ..blobs::Policy::default() };
    let scan = match blobs::check(&name, &bytes, &policy, blobs::clamd_socket_from_env().as_deref()) {
        Ok(scan) => scan,
        Err(rej) => {
            let reason = if rej.reason == "scan_unavailable" {
                RejectedReason::StoreUnavailable
            } else {
                RejectedReason::TypeNotAllowed
            };
            return (refused(&name, &shown_mime, len, reason), false);
        }
    };
    let stored = blobs::store_blob(state, tenant, &blob_name(key, index, &name), Some(&scan.mime), None, bytes).await;
    let out = match stored {
        Ok(v) => v,
        Err(e) => {
            let (reason, quota) = match &e {
                AppError::Structured { code: "blob_too_large", .. } => (RejectedReason::TooLarge, false),
                AppError::Structured { code: "quota_blob_bytes" | "quota_blobs", .. } => {
                    (RejectedReason::QuotaExceeded, true)
                }
                _ => (RejectedReason::StoreUnavailable, false),
            };
            return (refused(&name, &scan.mime, len, reason), quota);
        }
    };
    let Some(id) = out.get("id").and_then(Value::as_str).map(str::to_string) else {
        return (refused(&name, &scan.mime, len, RejectedReason::StoreUnavailable), false);
    };
    if state
        .db
        .blob_tag_email(tenant.id, id.clone(), key.to_string(), index as i64, declared_mime.clone(), part.inline)
        .await
        .is_err()
    {
        return (refused(&name, &scan.mime, len, RejectedReason::StoreUnavailable), false);
    }
    (stored_entry(state, tenant, &name, &id, &scan.mime, len, declared_mime, part.inline), false)
}

#[allow(clippy::too_many_arguments)]
fn stored_entry(
    state: &AppState,
    tenant: &Tenant,
    name: &str,
    blob_id: &str,
    mime: &str,
    bytes: i64,
    declared_mime: Option<String>,
    inline: bool,
) -> Value {
    let mut entry = json!({
        "name": name,
        "mime": mime,
        "bytes": bytes,
        "blob_id": blob_id,
        "url": url_for(state, tenant, blob_id),
        "inline": inline,
    });
    if let Some(d) = declared_mime.filter(|d| d != mime) {
        entry["declared_mime"] = json!(d);
    }
    entry
}

/// R1-R6: store every part, then write the one `email_received` message.
/// Returns `{message_id, attachments, data}` where `data` is what the inbox
/// message carries. A second delivery of the same `provider_message_id`
/// stores nothing new and reports the first message.
pub async fn receive(state: &AppState, tenant: &Tenant, mail: &InboundMail) -> Result<Value, AppError> {
    let key = message_key(&mail.provider_message_id);
    let mut attachments = Vec::with_capacity(mail.parts.len());
    let mut stored_any = false;
    let mut quota_hit = false;
    for (index, part) in mail.parts.iter().enumerate() {
        if index >= MAX_PARTS {
            let name = sanitise_filename(&part.filename, index, &part.content_type);
            let mime = declared(&part.content_type).unwrap_or_else(|| "application/octet-stream".to_string());
            attachments.push(refused(&name, &mime, base64_len_estimate(&part.content_base64), RejectedReason::TooManyParts));
            continue;
        }
        let (entry, quota) = decide_part(state, tenant, &key, index, part).await;
        stored_any |= entry.get("blob_id").is_some();
        quota_hit |= quota;
        attachments.push(entry);
    }

    let mut data = json!({
        "kind": "email_received",
        "provider_message_id": mail.provider_message_id,
        "from": mail.from,
        "subject": mail.subject,
        "attachments": attachments,
    });
    if stored_any || quota_hit {
        let (blobs_used, bytes_used) = state.db.blobs_usage_live(tenant.id, blobs::now(state)).await?;
        let plan = state
            .plans
            .get(&tenant.plan)
            .ok_or_else(|| AppError::Internal(format!("tenant plan '{}' not in catalog", tenant.plan)))?;
        data["quota"] = json!({
            "blobs": blobs_used,
            "blob_bytes": bytes_used,
            "blobs_max": plan.blobs_max,
            "blob_bytes_max": plan.blob_bytes_max,
        });
    }
    let body = format!("Mail from {}: {} ({} attachments)", mail.from, mail.subject, mail.parts.len());
    let (message_id, created) = state
        .db
        .insert_email_notice(
            tenant.id,
            format!("email:{}", mail.from),
            body,
            data.to_string(),
            mail.provider_message_id.clone(),
        )
        .await?;
    Ok(json!({"message_id": message_id, "created": created, "data": data}))
}
