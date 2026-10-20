//! PRD-mcphost-send-email-from-tool: `host.email.send` / `host.email.sends`
//! and the `mcphost.email.send` sandbox bridge -- one send verb for the agent
//! and for its tools, a tool-named sender, and a row per attempt.
//!
//! [`SendStatus`] is the only place the row statuses are written: the tool
//! description (`handler::host_tools`) renders [`SendStatus::names`], the
//! table's `status` column stores [`SendStatus::as_str`], and the drift test
//! compares the two.

use std::time::Duration;

use rusqlite::params;
use serde_json::{Value, json};

use crate::db::Tenant;
use crate::email::EmailMessage;
use crate::errors::AppError;
use crate::state::{AppState, new_ulid, now_unix};

/// Most recipients on one call.
pub const TO_MAX: usize = 10;
/// Subject bytes.
pub const SUBJECT_MAX_BYTES: usize = 998;
/// Plain-text body bytes (256 KiB).
pub const TEXT_MAX_BYTES: usize = 256 * 1024;
/// HTML body bytes (512 KiB).
pub const HTML_MAX_BYTES: usize = 512 * 1024;
/// Most rows one `host.email.sends` call returns.
pub const SENDS_MAX: i64 = 100;
/// The wait before the one retry of a provider failure (R8).
pub const RETRY_DELAY: Duration = Duration::from_secs(2);
/// The default sending domain (`MCPHOST_EMAIL_SEND_DOMAIN` overrides it).
pub const DEFAULT_SEND_DOMAIN: &str = "mcphost.dev";
/// The shared-domain fallback whose From rule is `<tool>+<slug>@<domain>`.
pub const FALLBACK_SEND_DOMAIN: &str = "mail.mcphost.dev";
/// The sender local part when the agent calls `host.email.send` itself.
pub const AGENT_SENDER: &str = "agent";
/// The key the bridge adds to the args it hands [`send_from_bridge`] so the
/// host side knows the calling tool; the sandbox cannot choose it.
pub const BRIDGE_TOOL_KEY: &str = "__tool";

/// How one send attempt ended. Order is the order every surface lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendStatus {
    Sent,
    ProviderError,
    QuotaExceeded,
    OwnerUnverified,
}

impl SendStatus {
    pub const ALL: &'static [SendStatus] =
        &[SendStatus::Sent, SendStatus::ProviderError, SendStatus::QuotaExceeded, SendStatus::OwnerUnverified];

    pub const fn as_str(self) -> &'static str {
        match self {
            SendStatus::Sent => "sent",
            SendStatus::ProviderError => "provider_error",
            SendStatus::QuotaExceeded => "quota_exceeded",
            SendStatus::OwnerUnverified => "owner_unverified",
        }
    }

    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|s| s.as_str()).collect()
    }
}

/// The sending domain: `MCPHOST_EMAIL_SEND_DOMAIN`, else [`DEFAULT_SEND_DOMAIN`].
pub fn send_domain() -> String {
    std::env::var("MCPHOST_EMAIL_SEND_DOMAIN")
        .ok()
        .map(|d| d.trim().to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| DEFAULT_SEND_DOMAIN.to_string())
}

/// The DNS-safe form of a tenant's namespace (`t_ab12` -> `t-ab12`).
pub fn tenant_slug(tenant: &Tenant) -> String {
    tenant.namespace.to_ascii_lowercase().replace('_', "-")
}

/// A tool name as a mail local part: lower-cased, anything outside
/// `[a-z0-9._-]` becomes `-`.
fn local_part(sender: &str) -> String {
    sender
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect()
}

/// R2: the From address, a function of the sender, the tenant slug and the
/// one env var: `<sender>@<slug>.<domain>`, or `<sender>+<slug>@<domain>` on
/// the shared fallback domain.
pub fn from_address(sender: &str, slug: &str, domain: &str) -> String {
    let local = local_part(sender);
    if domain == FALLBACK_SEND_DOMAIN {
        format!("{local}+{slug}@{domain}")
    } else {
        format!("{local}@{slug}.{domain}")
    }
}

/// One address, checked structurally: a single `@`, a short printable local
/// part, a dotted domain of plain labels.
pub fn valid_address(addr: &str) -> bool {
    if addr.len() > 254 {
        return false;
    }
    let Some((local, domain)) = addr.split_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || domain.contains('@') {
        return false;
    }
    if !local.chars().all(|c| c.is_ascii_alphanumeric() || "._%+-'".contains(c)) {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// The parsed, validated arguments of one send.
struct SendArgs {
    to: Vec<String>,
    subject: String,
    text: String,
    html: Option<String>,
    reply_to_message_id: Option<String>,
}

fn parse_args(args: &Value) -> Result<SendArgs, AppError> {
    let to: Vec<String> = match args.get("to") {
        Some(Value::String(s)) => vec![s.trim().to_string()],
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(|s| s.trim().to_string()).ok_or_else(|| AppError::InvalidArgs("to: every address must be a string".to_string())))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(AppError::InvalidArgs("to: must be an address or an array of addresses".to_string())),
        None => return Err(AppError::InvalidArgs("missing required argument 'to'".to_string())),
    };
    if to.is_empty() {
        return Err(AppError::InvalidArgs("to: at least one address is required".to_string()));
    }
    if to.len() > TO_MAX {
        return Err(AppError::InvalidArgs(format!("to: at most {TO_MAX} addresses, got {}", to.len())));
    }
    if let Some(bad) = to.iter().find(|a| !valid_address(a)) {
        return Err(AppError::InvalidArgs(format!("to: '{bad}' is not a valid email address")));
    }
    let mut deduped: Vec<String> = Vec::new();
    for addr in to {
        if !deduped.iter().any(|a| a.eq_ignore_ascii_case(&addr)) {
            deduped.push(addr);
        }
    }
    let req_str = |name: &str| -> Result<String, AppError> {
        args.get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
    };
    let subject = req_str("subject")?;
    if subject.trim().is_empty() || subject.contains(['\r', '\n']) {
        return Err(AppError::InvalidArgs("subject: must be non-empty and one line".to_string()));
    }
    if subject.len() > SUBJECT_MAX_BYTES {
        return Err(AppError::InvalidArgs(format!("subject: at most {SUBJECT_MAX_BYTES} bytes, got {}", subject.len())));
    }
    let text = req_str("text")?;
    if text.len() > TEXT_MAX_BYTES {
        return Err(AppError::InvalidArgs(format!("text: at most {TEXT_MAX_BYTES} bytes, got {}", text.len())));
    }
    let html = match args.get("html") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.len() <= HTML_MAX_BYTES => Some(s.clone()),
        Some(Value::String(s)) => {
            return Err(AppError::InvalidArgs(format!("html: at most {HTML_MAX_BYTES} bytes, got {}", s.len())));
        }
        Some(_) => return Err(AppError::InvalidArgs("html: must be a string".to_string())),
    };
    let reply_to_message_id = match args.get("reply_to_message_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.is_empty() && s.len() <= 998 && !s.contains(['\r', '\n']) => Some(s.clone()),
        Some(_) => return Err(AppError::InvalidArgs("reply_to_message_id: must be a one-line string".to_string())),
    };
    Ok(SendArgs { to: deduped, subject, text, html, reply_to_message_id })
}

/// One `email_sends` row to insert.
struct Row<'a> {
    id: &'a str,
    tool: &'a str,
    from: &'a str,
    to: &'a str,
    subject: &'a str,
    status: SendStatus,
    provider_message_id: Option<&'a str>,
    error: Option<&'a str>,
    reply_to_message_id: Option<&'a str>,
    sent_unix: i64,
}

async fn insert_row(state: &AppState, tenant_id: i64, row: Row<'_>) -> Result<(), AppError> {
    let (id, tool, from, to, subject) =
        (row.id.to_string(), row.tool.to_string(), row.from.to_string(), row.to.to_string(), row.subject.to_string());
    let (status, pmid, error, rtmid, sent) = (
        row.status.as_str(),
        row.provider_message_id.map(str::to_string),
        row.error.map(str::to_string),
        row.reply_to_message_id.map(str::to_string),
        row.sent_unix,
    );
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO email_sends (id, tenant_id, tool, from_addr, to_addrs, subject, status, \
                 provider_message_id, error, reply_to_message_id, sent_unix) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![id, tenant_id, tool, from, to, subject, status, pmid, error, rtmid, sent],
            )?;
            Ok(())
        })
        .await
}

/// `sent` rows this tenant has written at or after `since_unix`.
async fn sent_since(state: &AppState, tenant_id: i64, since_unix: i64) -> Result<i64, AppError> {
    state
        .db
        .with_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM email_sends WHERE tenant_id = ?1 AND status = 'sent' AND sent_unix >= ?2",
                params![tenant_id, since_unix],
                |r| r.get(0),
            )?)
        })
        .await
}

/// Sends once; one retry after [`RETRY_DELAY`] when the provider answered
/// 5xx or never answered (R8). A 4xx is the caller's mail and is not retried.
async fn send_with_retry(state: &AppState, message: &EmailMessage) -> Result<crate::email::SendReceipt, AppError> {
    match state.email_client.send_message(message).await {
        Ok(receipt) => Ok(receipt),
        Err(first) => {
            let retryable = first.claim_email_status().is_none_or(|s| s >= 500);
            if !retryable {
                return Err(first);
            }
            tokio::time::sleep(RETRY_DELAY).await;
            state.email_client.send_message(message).await
        }
    }
}

/// `host.email.send` and the bridge's op: `tool` is the calling tool run's
/// name, `None` for the agent itself.
pub async fn send(state: &AppState, tenant: &Tenant, tool: Option<&str>, args: &Value) -> Result<Value, AppError> {
    let parsed = parse_args(args)?;
    let sender = tool.filter(|t| !t.is_empty()).unwrap_or(AGENT_SENDER);
    let from = from_address(sender, &tenant_slug(tenant), &send_domain());
    // R2: replies go to the tenant's inbound address (`inbox@` on the same
    // sending domain, where `email_inbound` receives them).
    let reply_to = from_address("inbox", &tenant_slug(tenant), &send_domain());

    // R5: until the owner has verified, the owner's own address is the only
    // recipient -- the anti-spam line. A refusal is a row and sends nothing.
    if tenant.owner_verified_at.is_none() {
        let owner = tenant.owner_email.as_deref();
        let stranger = parsed.to.iter().find(|a| !owner.is_some_and(|o| o.eq_ignore_ascii_case(a)));
        if let Some(stranger) = stranger {
            let to_joined = parsed.to.join(", ");
            let id = format!("em_{}", new_ulid().to_ascii_lowercase());
            insert_row(
                state,
                tenant.id,
                Row {
                    id: &id,
                    tool: sender,
                    from: &from,
                    to: &to_joined,
                    subject: &parsed.subject,
                    status: SendStatus::OwnerUnverified,
                    provider_message_id: None,
                    error: None,
                    reply_to_message_id: parsed.reply_to_message_id.as_deref(),
                    sent_unix: now_unix(),
                },
            )
            .await?;
            return Err(AppError::Structured {
                code: EMAIL_OWNER_UNVERIFIED,
                message: format!("'{stranger}' is not this tenant's owner and the owner has not verified"),
                data: json!({
                    "to": stranger,
                    "next_call": "host.claim",
                    "remedy": "Until the owner verifies (host.claim returns the claim link), mail goes only to the owner's own address.",
                }),
            });
        }
    }

    // R4: the plan's daily cap, counted per tenant per UTC day over `sent`
    // rows. A refused attempt is a `quota_exceeded` row and no provider call.
    let limit = state
        .plans
        .get(&tenant.plan)
        .map(|p| p.emails_per_day)
        .ok_or_else(|| AppError::Internal(format!("tenant plan '{}' not in catalog", tenant.plan)))?;
    let now = now_unix();
    let midnight = crate::state::utc_midnight_unix(now);
    let used = sent_since(state, tenant.id, midnight).await?;
    if used + parsed.to.len() as i64 > limit {
        let to_joined = parsed.to.join(", ");
        let id = format!("em_{}", new_ulid().to_ascii_lowercase());
        insert_row(
            state,
            tenant.id,
            Row {
                id: &id,
                tool: sender,
                from: &from,
                to: &to_joined,
                subject: &parsed.subject,
                status: SendStatus::QuotaExceeded,
                provider_message_id: None,
                error: None,
                reply_to_message_id: parsed.reply_to_message_id.as_deref(),
                sent_unix: now,
            },
        )
        .await?;
        return Err(AppError::Structured {
            code: EMAIL_QUOTA_EXCEEDED,
            message: format!("{used} emails sent today; this plan allows {limit} (emails_per_day)"),
            data: json!({
                "used": used,
                "limit": limit,
                "resets_at": crate::state::rfc3339_from_unix(midnight + 86_400),
                "remedy": "/help/email_quota_exceeded",
            }),
        });
    }

    let mut sends = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut last_status: Option<u16> = None;
    for to in &parsed.to {
        let message = EmailMessage {
            to: to.clone(),
            from: from.clone(),
            reply_to: reply_to.clone(),
            subject: parsed.subject.clone(),
            text_body: parsed.text.clone(),
            html_body: parsed.html.clone(),
            headers: parsed
                .reply_to_message_id
                .iter()
                .flat_map(|id| [("In-Reply-To".to_string(), id.clone()), ("References".to_string(), id.clone())])
                .collect(),
            attachments: Vec::new(),
        };
        let id = format!("em_{}", new_ulid().to_ascii_lowercase());
        let now = now_unix();
        match send_with_retry(state, &message).await {
            Ok(receipt) => {
                insert_row(
                    state,
                    tenant.id,
                    Row {
                        id: &id,
                        tool: sender,
                        from: &from,
                        to,
                        subject: &parsed.subject,
                        status: SendStatus::Sent,
                        provider_message_id: receipt.provider_message_id.as_deref(),
                        error: None,
                        reply_to_message_id: parsed.reply_to_message_id.as_deref(),
                        sent_unix: now,
                    },
                )
                .await?;
                sends.push(drop_nulls(json!({"id": id, "to": to, "provider_message_id": receipt.provider_message_id, "sent_at": now})));
            }
            Err(e) => {
                let status = e.claim_email_status();
                last_status = status;
                let detail = status.map_or_else(|| "no response".to_string(), |s| s.to_string());
                insert_row(
                    state,
                    tenant.id,
                    Row {
                        id: &id,
                        tool: sender,
                        from: &from,
                        to,
                        subject: &parsed.subject,
                        status: SendStatus::ProviderError,
                        provider_message_id: None,
                        error: Some(&detail),
                        reply_to_message_id: parsed.reply_to_message_id.as_deref(),
                        sent_unix: now,
                    },
                )
                .await?;
                failed.push(json!({"id": id, "to": to, "status": status}));
            }
        }
    }
    if !failed.is_empty() {
        return Err(AppError::Structured {
            code: EMAIL_PROVIDER_ERROR,
            message: "the email provider did not accept the message".to_string(),
            data: json!({"status": last_status, "failed": failed, "sent": sends}),
        });
    }
    let first = sends.first().cloned().unwrap_or(Value::Null);
    Ok(drop_nulls(json!({
        "id": first["id"],
        "provider_message_id": first["provider_message_id"],
        "from": from,
        "sent_at": first["sent_at"],
        "sends": sends,
    })))
}

/// An absent value is an absent key, never a `null` (the output schema types
/// each key as the thing itself).
fn drop_nulls(mut value: Value) -> Value {
    if let Some(obj) = value.as_object_mut() {
        obj.retain(|_, v| !v.is_null());
    }
    value
}

/// The bridge's op: the sandbox's args plus the host-added [`BRIDGE_TOOL_KEY`].
pub async fn send_from_bridge(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let mut args = args.clone();
    let tool = args.as_object_mut().and_then(|o| o.remove(BRIDGE_TOOL_KEY)).and_then(|v| v.as_str().map(str::to_string));
    send(state, tenant, tool.as_deref(), &args).await
}

/// `host.email.sends {limit}`: this tenant's rows, newest first.
pub async fn sends(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    let limit = match args.get("limit") {
        None | Some(Value::Null) => 20,
        Some(v) => v
            .as_i64()
            .filter(|n| (1..=SENDS_MAX).contains(n))
            .ok_or_else(|| AppError::InvalidArgs(format!("limit: an integer from 1 to {SENDS_MAX}")))?,
    };
    let tenant_id = tenant.id;
    let rows = state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, tool, from_addr, to_addrs, subject, status, provider_message_id, error, sent_unix \
                 FROM email_sends WHERE tenant_id = ?1 ORDER BY sent_unix DESC, rowid DESC LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![tenant_id, limit], |r| {
                    Ok(drop_nulls(json!({
                        "id": r.get::<_, String>(0)?,
                        "tool": r.get::<_, String>(1)?,
                        "from": r.get::<_, String>(2)?,
                        "to": r.get::<_, String>(3)?,
                        "subject": r.get::<_, String>(4)?,
                        "status": r.get::<_, String>(5)?,
                        "provider_message_id": r.get::<_, Option<String>>(6)?,
                        "error": r.get::<_, Option<String>>(7)?,
                        "sent_at": r.get::<_, i64>(8)?,
                    })))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(json!({"sends": rows}))
}

crate::err_code! {
    pub EMAIL_OWNER_UNVERIFIED,
    code: "email_owner_unverified", family: Auth, class: HostRefusal, retryable: false,
    remedy: "Verify the owner email first (data.next_call is host.claim); until then mail goes only to the owner's own address.",
}

crate::err_code! {
    pub EMAIL_QUOTA_EXCEEDED,
    code: "email_quota_exceeded", family: Plan, class: PlanLimit, retryable: true,
    remedy: "The daily email quota is used; tell your user, and retry after data.resets_at or move to a plan with a higher emails_per_day.",
}

crate::err_code! {
    pub EMAIL_PROVIDER_ERROR,
    code: "email_provider_error", family: Internal, class: HostFault, retryable: true,
    remedy: "The email provider refused or never answered after one retry; data.status is its HTTP status. Retry later.",
}
