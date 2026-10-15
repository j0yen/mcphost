//! PRD-mcphost-inbound-email-address-per-tenant: every tenant has an
//! address (`host.email.address`), and a mail sent to it is one parsed inbox
//! message (`POST /email/inbound`).
//!
//! The provider's webhook is verified through [`InboundVerifier`] (Resend's
//! Svix signature, Postmark's basic auth, a fake for tests); every impl
//! returns the same [`InboundEvent`], so parsing happens once. The message
//! lands through [`crate::db::Db::insert_host_notice`], the same store
//! `host.msg.send` writes, so `host.msg.wait` / `host.msg.inbox` return it
//! unchanged.

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

use crate::db::Tenant;
use crate::email::EmailProvider;
use crate::errors::AppError;
use crate::state::AppState;

/// Default `MCPHOST_EMAIL_INBOUND_DOMAIN`.
pub const DEFAULT_DOMAIN: &str = "in.mcphost.dev";

/// Svix timestamp tolerance (R2: five minutes).
pub const TIMESTAMP_WINDOW_SECS: i64 = 300;

/// `text` / `html_stripped` cap (R3).
pub const TEXT_CAP_BYTES: usize = 64 * 1024;

/// The `from_address` an inbound mail is delivered under.
pub const SENDER: &str = "host.email";

/// R4: every inbox `data.kind` this fleet writes for email. The tool
/// description renders from [`EmailInboxKind::ALL`], and a unit test asserts
/// every variant appears in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmailInboxKind {
    EmailReceived,
    EmailInboundQuotaExceeded,
    EmailBounced,
}

impl EmailInboxKind {
    pub const ALL: &'static [EmailInboxKind] = &[
        EmailInboxKind::EmailReceived,
        EmailInboxKind::EmailInboundQuotaExceeded,
        EmailInboxKind::EmailBounced,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EmailInboxKind::EmailReceived => "email_received",
            EmailInboxKind::EmailInboundQuotaExceeded => "email_inbound_quota_exceeded",
            EmailInboxKind::EmailBounced => "email_bounced",
        }
    }

    /// `"email_received, email_inbound_quota_exceeded, email_bounced"`.
    pub fn rendered() -> String {
        Self::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
    }
}

/// `host.email.address`'s tool description, rendered from
/// [`EmailInboxKind`].
pub fn address_tool_description() -> String {
    format!(
        "Return this tenant's inbound email address: {{address, domain, enabled}}. Mail sent to \
         it arrives as a message in host.msg.inbox / host.msg.wait whose data.kind is one of: {}. \
         email_received carries {{message_id, from, to, subject, text, html_stripped, received_at, \
         attachments, truncated}}. The address is the same on every call; enabled is false until \
         the host has an inbound signing secret.",
        EmailInboxKind::rendered()
    )
}

/// Inbound half of [`crate::email::EmailConfig`]: `MCPHOST_EMAIL_INBOUND_SECRET`,
/// `MCPHOST_EMAIL_INBOUND_DOMAIN`, and `MCPHOST_EMAIL_PROVIDER=fake`.
#[derive(Clone, Debug, Default)]
pub struct InboundConfig {
    pub secret: Option<String>,
    pub domain: Option<String>,
    /// `MCPHOST_EMAIL_PROVIDER=fake`: verify with [`FakeInbound`].
    pub fake: bool,
}

impl InboundConfig {
    pub fn from_env() -> Self {
        Self {
            secret: std::env::var("MCPHOST_EMAIL_INBOUND_SECRET").ok().filter(|s| !s.is_empty()),
            domain: std::env::var("MCPHOST_EMAIL_INBOUND_DOMAIN").ok().filter(|s| !s.is_empty()),
            fake: std::env::var("MCPHOST_EMAIL_PROVIDER").ok().as_deref() == Some("fake"),
        }
    }

    pub fn domain(&self) -> &str {
        self.domain.as_deref().unwrap_or(DEFAULT_DOMAIN)
    }

    /// The route answers 503 `email_inbound_disabled` without a secret.
    pub fn enabled(&self) -> bool {
        self.secret.is_some()
    }
}

/// R1: `host.email.address`.
pub fn address(state: &AppState, tenant: &Tenant) -> Value {
    let inbound = &state.email_config.inbound;
    json!({
        "address": format!("{}@{}", tenant.namespace, inbound.domain()),
        "domain": inbound.domain(),
        "enabled": inbound.enabled(),
    })
}

/// One parsed inbound mail, whichever provider sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundEvent {
    pub message_id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub text: String,
    pub html: String,
    pub attachments: Vec<Value>,
}

pub fn signature_invalid(message: impl Into<String>) -> AppError {
    AppError::Structured { code: "email_signature_invalid", message: message.into(), data: json!({}) }
}

/// R2: verify one provider's webhook and parse it.
pub trait InboundVerifier: Send + Sync {
    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<InboundEvent, AppError>;
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn str_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or_default().to_string()
}

fn addresses_of(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => s.split(',').map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// Resend-shaped payload (fields may sit under `data`) or the generic
/// `{from, to, subject, text, html}` the fake accepts.
fn parse_resend_shaped(body: &[u8], fallback_id: &str) -> Result<InboundEvent, AppError> {
    let v: Value = serde_json::from_slice(body).map_err(|e| AppError::InvalidArgs(format!("body: not JSON: {e}")))?;
    let d = v.get("data").filter(|d| d.is_object()).unwrap_or(&v);
    let id = ["message_id", "email_id", "id"]
        .iter()
        .find_map(|k| d.get(*k).and_then(Value::as_str))
        .unwrap_or(fallback_id)
        .to_string();
    let attachments = d
        .get("attachments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    json!({
                        "name": x.get("filename").or_else(|| x.get("name")).cloned().unwrap_or(Value::Null),
                        "mime": x.get("content_type").or_else(|| x.get("mime")).cloned().unwrap_or(Value::Null),
                        "bytes": x.get("size").or_else(|| x.get("bytes")).cloned().unwrap_or(json!(0)),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(InboundEvent {
        message_id: id,
        from: str_of(d.get("from")),
        to: addresses_of(d.get("to")),
        subject: str_of(d.get("subject")),
        text: str_of(d.get("text")),
        html: str_of(d.get("html")),
        attachments,
    })
}

/// Resend (Svix): `svix-id` / `svix-timestamp` / `svix-signature`, HMAC-SHA256
/// over `id.timestamp.body`, base64, `v1,<sig>` entries space-separated. A
/// `whsec_`-prefixed secret is the base64 key Svix issues; any other secret
/// is used as its raw bytes.
pub struct ResendInbound {
    pub secret: String,
    pub now_unix: fn() -> i64,
}

impl ResendInbound {
    pub fn new(secret: &str) -> Self {
        Self { secret: secret.to_string(), now_unix: crate::state::now_unix }
    }

    fn key(&self) -> Vec<u8> {
        match self.secret.strip_prefix("whsec_") {
            Some(b64) => B64.decode(b64).unwrap_or_else(|_| self.secret.as_bytes().to_vec()),
            None => self.secret.as_bytes().to_vec(),
        }
    }

    /// The `v1,<base64>` signature a Svix sender produces (tests sign with it).
    pub fn sign(&self, id: &str, timestamp: i64, body: &[u8]) -> String {
        let signed = [id.as_bytes(), b".", timestamp.to_string().as_bytes(), b".", body].concat();
        format!("v1,{}", B64.encode(crate::billing::hmac_sha256(&self.key(), &signed)))
    }
}

impl InboundVerifier for ResendInbound {
    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<InboundEvent, AppError> {
        let (Some(id), Some(ts), Some(sigs)) =
            (header(headers, "svix-id"), header(headers, "svix-timestamp"), header(headers, "svix-signature"))
        else {
            return Err(signature_invalid("svix-id, svix-timestamp and svix-signature are required"));
        };
        let Ok(timestamp) = ts.parse::<i64>() else {
            return Err(signature_invalid("svix-timestamp is not a unix time"));
        };
        if ((self.now_unix)() - timestamp).abs() > TIMESTAMP_WINDOW_SECS {
            return Err(signature_invalid("svix-timestamp is outside the 5-minute window"));
        }
        let expected = self.sign(id, timestamp, body);
        let matched = sigs
            .split_whitespace()
            .any(|s| crate::auth::constant_time_eq(s.as_bytes(), expected.as_bytes()));
        if !matched {
            return Err(signature_invalid("svix-signature does not match"));
        }
        parse_resend_shaped(body, id)
    }
}

/// Postmark: HTTP basic auth whose password is the secret.
pub struct PostmarkInbound {
    pub secret: String,
}

impl InboundVerifier for PostmarkInbound {
    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<InboundEvent, AppError> {
        let creds = header(headers, "authorization")
            .and_then(|h| h.strip_prefix("Basic "))
            .and_then(|b| B64.decode(b.trim()).ok())
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| signature_invalid("basic auth is required"))?;
        let password = creds.split_once(':').map(|(_, p)| p).unwrap_or_default();
        if !crate::auth::constant_time_eq(password.as_bytes(), self.secret.as_bytes()) {
            return Err(signature_invalid("basic auth does not match"));
        }
        let v: Value =
            serde_json::from_slice(body).map_err(|e| AppError::InvalidArgs(format!("body: not JSON: {e}")))?;
        let attachments = v
            .get("Attachments")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|x| {
                        json!({
                            "name": x.get("Name").cloned().unwrap_or(Value::Null),
                            "mime": x.get("ContentType").cloned().unwrap_or(Value::Null),
                            "bytes": x.get("ContentLength").cloned().unwrap_or(json!(0)),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(InboundEvent {
            message_id: str_of(v.get("MessageID")),
            from: str_of(v.get("From")),
            to: addresses_of(v.get("To")),
            subject: str_of(v.get("Subject")),
            text: str_of(v.get("TextBody")),
            html: str_of(v.get("HtmlBody")),
            attachments,
        })
    }
}

/// Test-support verifier (`MCPHOST_EMAIL_PROVIDER=fake`): the shared secret
/// travels in `x-mcphost-fake-inbound`, the body is the Resend-shaped JSON.
pub struct FakeInbound {
    pub secret: String,
}

impl InboundVerifier for FakeInbound {
    fn verify(&self, headers: &HeaderMap, body: &[u8]) -> Result<InboundEvent, AppError> {
        match header(headers, "x-mcphost-fake-inbound") {
            Some(s) if crate::auth::constant_time_eq(s.as_bytes(), self.secret.as_bytes()) => {
                parse_resend_shaped(body, "fake-inbound")
            }
            _ => Err(signature_invalid("x-mcphost-fake-inbound does not match")),
        }
    }
}

/// Selection by [`EmailProvider`] (and the `fake` flag), as R2 requires.
pub fn verifier_for(provider: EmailProvider, cfg: &InboundConfig, secret: &str) -> Box<dyn InboundVerifier> {
    if cfg.fake {
        return Box::new(FakeInbound { secret: secret.to_string() });
    }
    match provider {
        EmailProvider::Resend => Box::new(ResendInbound::new(secret)),
        EmailProvider::Postmark => Box::new(PostmarkInbound { secret: secret.to_string() }),
    }
}

/// Cuts `s` to at most [`TEXT_CAP_BYTES`] on a char boundary; the flag says
/// whether anything was cut.
pub fn cap(s: &str) -> (String, bool) {
    if s.len() <= TEXT_CAP_BYTES {
        return (s.to_string(), false);
    }
    let mut end = TEXT_CAP_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// Tags removed, `<style>` / `<script>` bodies dropped, the five common
/// entities decoded -- a small state machine, no crate.
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let skip_to = ["style", "script"].iter().find_map(|tag| {
                let open = format!("<{tag}");
                if lower[i..].starts_with(&open)
                    && lower[i + open.len()..].starts_with(|c: char| c == '>' || c == ' ' || c == '/' || c == '\t' || c == '\n')
                {
                    let close = format!("</{tag}");
                    Some(lower[i..].find(&close).map(|p| i + p).unwrap_or(bytes.len()))
                } else {
                    None
                }
            });
            if let Some(at) = skip_to {
                i = at;
            }
            match html[i..].find('>') {
                Some(p) => i += p + 1,
                None => break,
            }
            continue;
        }
        let ch = html[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The local part of the first `to` address whose domain is `domain`.
fn recipient_slug(to: &[String], domain: &str) -> Option<String> {
    to.iter().find_map(|raw| {
        let addr = crate::email::reply_to_address(raw).to_ascii_lowercase();
        let (local, host) = addr.rsplit_once('@')?;
        (host == domain.to_ascii_lowercase()).then(|| local.to_string())
    })
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (status, axum::Json(json!({"error_code": code, "error": message}))).into_response()
}

/// `POST /email/inbound` (R2, R3, R6, R7).
pub async fn receive(state: std::sync::Arc<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let cfg = &state.email_config.inbound;
    let Some(secret) = cfg.secret.as_deref() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "email_inbound_disabled", "inbound email is not configured");
    };
    let verifier = verifier_for(state.email_config.provider, cfg, secret);
    let event = match verifier.verify(&headers, &body) {
        Ok(e) => e,
        Err(AppError::Structured { code: "email_signature_invalid", message, .. }) => {
            return error_response(StatusCode::UNAUTHORIZED, "email_signature_invalid", &message);
        }
        Err(e) => return error_response(StatusCode::BAD_REQUEST, "args_invalid", &e.to_string()),
    };
    let bytes = body.len() as i64;
    let tenant = match recipient_slug(&event.to, cfg.domain()) {
        Some(slug) => match state.db.find_tenant_by_namespace(slug).await {
            Ok(t) => t.filter(|t| !t.disabled),
            Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "storage", &e.to_string()),
        },
        None => None,
    };
    // R6: never a retry storm -- 200 to the provider, a log row, no inbox write.
    let Some(tenant) = tenant else {
        let _ = state
            .db
            .insert_email_inbound_log(None, event.message_id.clone(), "unknown_recipient", bytes)
            .await;
        return (StatusCode::OK, axum::Json(json!({"ok": true, "delivered": false}))).into_response();
    };
    let (text, text_cut) = cap(&event.text);
    let (html_stripped, html_cut) = cap(&strip_html(&event.html));
    let data = json!({
        "kind": EmailInboxKind::EmailReceived.as_str(),
        "message_id": event.message_id,
        "from": event.from,
        "to": event.to,
        "subject": event.subject,
        "text": text,
        "html_stripped": html_stripped,
        "received_at": crate::state::now_unix(),
        "attachments": event.attachments,
        "truncated": text_cut || html_cut,
    });
    let written = state
        .db
        .insert_host_notice(
            tenant.id,
            format!("email-inbound-{}", tenant.id),
            SENDER.to_string(),
            event.subject.clone(),
            data.to_string(),
        )
        .await;
    if let Err(e) = written {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "storage", &e.to_string());
    }
    let _ = state
        .db
        .insert_email_inbound_log(Some(tenant.id), event.message_id.clone(), "accepted", bytes)
        .await;
    (StatusCode::OK, axum::Json(json!({"ok": true, "delivered": true}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_renders_every_inbox_kind() {
        let d = address_tool_description();
        for k in EmailInboxKind::ALL {
            assert!(d.contains(k.as_str()), "{} missing from {d}", k.as_str());
        }
    }

    #[test]
    fn strip_html_drops_tags_style_and_decodes_entities() {
        assert_eq!(
            strip_html("<p>Invoice &amp; <b>4421</b></p><style>p{x:y}</style><script>1<2</script>done &lt;ok&gt;"),
            "Invoice & 4421done <ok>"
        );
    }

    #[test]
    fn cap_cuts_on_a_char_boundary() {
        let s = "é".repeat(TEXT_CAP_BYTES);
        let (cut, flag) = cap(&s);
        assert!(flag && cut.len() <= TEXT_CAP_BYTES);
    }
}
