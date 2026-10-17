//! PRD-mcphost-notify-user-channel: `host.notify.*` -- a tenant registers
//! where its human lives once (`channel_add`), and `send` reaches them there
//! with a per-channel receipt.
//!
//! [`ChannelKind`] and [`Level`] are the only places the member names are
//! written. The tool schemas (`handler::host_tools`), the llms.txt section
//! ([`render_section`]), the delivery dispatch ([`dispatch_kinds`]) and the
//! `/help/notify_kind_unknown` page ([`expand_placeholders`]) all read them
//! from here, and [`check_exact`] is the drift test's comparison.

use std::time::Duration;

use rusqlite::params;
use serde_json::{Value, json};

use crate::db::Tenant;
use crate::errors::AppError;
use crate::state::{AppState, new_ulid, now_unix};

/// The `/help/<code>` page for an unknown `kind`.
pub const KIND_UNKNOWN_CODE: &str = "notify_kind_unknown";

/// Where a channel delivers. Order is the order every surface lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    Email,
    Webhook,
    Slack,
}

impl ChannelKind {
    pub const ALL: &'static [ChannelKind] = &[ChannelKind::Email, ChannelKind::Webhook, ChannelKind::Slack];

    pub const fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Email => "email",
            ChannelKind::Webhook => "webhook",
            ChannelKind::Slack => "slack",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|k| k.as_str()).collect()
    }
}

/// How urgent a notification is. Order is the order every surface lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Action,
}

impl Level {
    pub const ALL: &'static [Level] = &[Level::Info, Level::Warn, Level::Action];

    pub const fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Action => "action",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|l| l.as_str() == s)
    }

    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().map(|l| l.as_str()).collect()
    }
}

/// How one [`ChannelKind`] is delivered. The exhaustive match in [`route`]
/// is the delivery dispatch: a new variant does not compile until it names a
/// transport, and [`dispatch_kinds`] reads the names back out of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// The claim-email client (`email.rs`).
    EmailClient,
    /// A JSON POST of the full envelope to the target URL.
    JsonPost,
    /// A JSON POST of `{text}` to a Slack incoming-webhook URL.
    SlackPost,
}

pub const fn route(kind: ChannelKind) -> (&'static str, Transport) {
    match kind {
        ChannelKind::Email => (ChannelKind::Email.as_str(), Transport::EmailClient),
        ChannelKind::Webhook => (ChannelKind::Webhook.as_str(), Transport::JsonPost),
        ChannelKind::Slack => (ChannelKind::Slack.as_str(), Transport::SlackPost),
    }
}

/// The kind names the delivery dispatch handles, in dispatch order.
pub fn dispatch_kinds() -> Vec<&'static str> {
    ChannelKind::ALL.iter().map(|k| route(*k).0).collect()
}

/// The drift comparison: `rendered` (what `surface` lists) must equal
/// `expected` exactly, in order.
pub fn check_exact(surface: &str, rendered: &[String], expected: &[&str]) -> Result<(), String> {
    if rendered.len() == expected.len() && rendered.iter().zip(expected).all(|(a, b)| a == b) {
        Ok(())
    } else {
        Err(format!("{surface} lists {rendered:?}, the enum has {expected:?}"))
    }
}

fn code_list(names: &[&str]) -> String {
    names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
}

/// Substitutes the generated enum lists into a static help string:
/// `{notify_kinds}` and `{notify_levels}`.
pub fn expand_placeholders(text: &str) -> String {
    text.replace("{notify_kinds}", &ChannelKind::names().join(", "))
        .replace("{notify_levels}", &Level::names().join(", "))
}

pub const SECTION_START: &str = "<!-- notify:start -->";
pub const SECTION_END: &str = "<!-- notify:end -->";

/// The `## Notify your user` llms.txt block, rendered from the enums.
pub fn render_section() -> String {
    format!(
        "{SECTION_START}\n## Notify your user\n\n\
         Register where your user lives once with `host.notify.channel_add {{kind, target}}` \
         (kind is one of {}), then reach them with `host.notify.send {{text, level, link?}}` \
         (level is one of {}); `host.notify.receipts` shows per-channel delivery. \
         An unknown kind answers `notify_kind_unknown` (`/help/notify_kind_unknown`).\n{SECTION_END}",
        code_list(&ChannelKind::names()),
        code_list(&Level::names()),
    )
}

/// Replace the notify block in `content`, or -- on a first run -- insert it
/// just before `## Limits and pricing` (else append).
pub fn splice_section(content: &str, section: &str) -> String {
    let section = section.trim_end();
    if let (Some(start), Some(end_pos)) = (content.find(SECTION_START), content.find(SECTION_END)) {
        let end = end_pos + SECTION_END.len();
        format!("{}{}{}", &content[..start], section, &content[end..])
    } else if let Some(idx) = content.find("## Limits and pricing") {
        format!("{}{}\n\n{}", &content[..idx], section, &content[idx..])
    } else {
        format!("{}\n\n{}\n", content.trim_end(), section)
    }
}

/// The verbs the host serves; the dispatch in `handler.rs` and the tool
/// list read this.
pub const VERBS: &[&str] = &[
    "host.notify.channel_add",
    "host.notify.channel_list",
    "host.notify.channel_remove",
    "host.notify.send",
    "host.notify.receipts",
];

pub fn kind_unknown(got: &str) -> AppError {
    AppError::Structured {
        code: KIND_UNKNOWN_CODE,
        message: format!("kind '{got}' is not a channel kind; one of {}", ChannelKind::names().join(", ")),
        data: json!({"field": "kind", "got": got, "kinds": ChannelKind::names()}),
    }
}

pub fn level_unknown(got: &str) -> AppError {
    AppError::InvalidArgs(format!("level: '{got}' is not one of {}", Level::names().join(", ")))
}

/// The longest `text` a send carries.
pub const TEXT_MAX_BYTES: usize = 1024;

/// Delivery attempts per (notify, channel) and the wait before each retry.
pub const ATTEMPTS_MAX: usize = 3;
pub const RETRY_BACKOFF: [Duration; 2] = [Duration::from_secs(10), Duration::from_secs(60)];

/// `MCPHOST_NOTIFY_RETRY_BACKOFF_MS=a,b` replaces [`RETRY_BACKOFF`] (a fixture
/// cannot wait 70 s); unset or malformed keeps the default.
fn retry_backoff() -> [Duration; 2] {
    let parsed = std::env::var("MCPHOST_NOTIFY_RETRY_BACKOFF_MS").ok().and_then(|v| {
        let (a, b) = v.split_once(',')?;
        Some([Duration::from_millis(a.trim().parse().ok()?), Duration::from_millis(b.trim().parse().ok()?)])
    });
    parsed.unwrap_or(RETRY_BACKOFF)
}

/// Each outbound request's ceiling.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// A channel that fails this many sends in a row is disabled.
pub const DISABLE_AFTER_FAILURES: i64 = 10;

/// The rolling window `notifies_per_day` counts over.
const DAY_SECS: i64 = 24 * 3600;

fn arg_str(args: &Value, name: &str) -> Result<String, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::InvalidArgs(format!("missing required argument '{name}'")))
}

fn check_args(state: &AppState, args: &Value, verb: &str) -> Result<(), AppError> {
    if let Some(known) = crate::handler::known_control_plane_args(verb, &state.kinds) {
        crate::errors::check_unknown_argument(args, verb, &known)?;
    }
    Ok(())
}

fn plan_for<'a>(state: &'a AppState, tenant: &Tenant) -> Result<&'a crate::plans::Plan, AppError> {
    state
        .plans
        .get(&tenant.plan)
        .ok_or_else(|| AppError::Internal(format!("tenant plan '{}' not in catalog", tenant.plan)))
}

fn target_invalid(why: &str) -> AppError {
    AppError::Structured {
        code: NOTIFY_TARGET_INVALID,
        message: format!("target: {why}"),
        data: json!({"field": "target"}),
    }
}

/// Whether `url` may be posted to: https (plain http only in test builds,
/// like the other host-side fetches) and a host that is neither private,
/// loopback, link-local nor this host's own domain -- checked on the literal
/// and on every address the name resolves to.
async fn url_allowed(state: &AppState, url: &str) -> Result<(), String> {
    use crate::kinds::http::{HickoryLookup, NameLookup, is_disallowed_ip, is_disallowed_literal_host, own_domain_from_url};

    let allow_loopback = cfg!(feature = "test-support");
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("not a valid URL: {e}"))?;
    let scheme_ok = parsed.scheme() == "https" || (allow_loopback && parsed.scheme() == "http");
    if !scheme_ok {
        return Err("must be an https URL".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "has no host".to_string())?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    if is_disallowed_literal_host(&host, &own_domain_from_url(&state.public_url), allow_loopback) {
        return Err(format!("host '{host}' is private, loopback, internal or this host's own domain"));
    }
    if host.parse::<std::net::IpAddr>().is_err() {
        let lookup = HickoryLookup::new().map_err(|e| format!("dns: {e}"))?;
        let ips = lookup.lookup(host.clone()).await.map_err(|e| format!("dns lookup of '{host}' failed: {e}"))?;
        if ips.is_empty() || ips.iter().any(|ip| is_disallowed_ip(*ip, allow_loopback)) {
            return Err(format!("host '{host}' resolves to a private or unresolvable address"));
        }
    }
    Ok(())
}

/// The part of a stored target that may be shown back: a URL's host only (the
/// path of a Slack incoming webhook is its secret).
fn hint_for(kind: ChannelKind, target: &str) -> String {
    match kind {
        ChannelKind::Email => target.to_string(),
        ChannelKind::Webhook | ChannelKind::Slack => reqwest::Url::parse(target)
            .ok()
            .and_then(|u| u.host_str().map(|h| format!("{h}/...")))
            .unwrap_or_else(|| "...".to_string()),
    }
}

/// An email channel may only point at the tenant's claim-verified owner
/// address. Read fresh: a claim completing between two calls must be seen.
async fn check_owner_email(state: &AppState, tenant: &Tenant, target: &str) -> Result<(), AppError> {
    let fresh = state
        .db
        .find_tenant_by_namespace(tenant.namespace.clone())
        .await?
        .ok_or_else(|| AppError::Internal("tenant vanished mid-call".to_string()))?;
    let Some(owner) = fresh.owner_email.as_deref().filter(|_| fresh.owner_verified_at.is_some()) else {
        return Err(AppError::Structured {
            code: OWNER_UNVERIFIED,
            message: "email channels go to the owner's verified address; this tenant's owner has not verified one".to_string(),
            data: json!({"next_call": "host.claim"}),
        });
    };
    if !owner.trim().eq_ignore_ascii_case(target.trim()) {
        return Err(AppError::Structured {
            code: EMAIL_NOT_OWNER,
            message: "an email channel may only target the verified owner address".to_string(),
            data: json!({"field": "target"}),
        });
    }
    Ok(())
}

/// `host.notify.channel_add {kind, target}`.
pub async fn channel_add(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_args(state, args, "host.notify.channel_add")?;
    let kind_name = arg_str(args, "kind")?;
    let kind = ChannelKind::parse(&kind_name).ok_or_else(|| kind_unknown(&kind_name))?;
    let target = arg_str(args, "target")?.trim().to_string();
    if target.is_empty() {
        return Err(target_invalid("must be non-empty"));
    }
    match route(kind).1 {
        Transport::EmailClient => check_owner_email(state, tenant, &target).await?,
        Transport::JsonPost | Transport::SlackPost => {
            url_allowed(state, &target).await.map_err(|why| target_invalid(&why))?;
        }
    }

    let max = plan_for(state, tenant)?.notify_channels_max;
    let (target_enc, target_nonce) = state.secrets.encrypt(&target)?;
    let id = new_ulid().to_ascii_lowercase();
    let hint = hint_for(kind, &target);
    let (tenant_id, row_id, row_hint) = (tenant.id, id.clone(), hint.clone());
    state
        .db
        .with_conn(move |conn| {
            let held: i64 = conn.query_row(
                "SELECT COUNT(*) FROM notify_channels WHERE tenant_id = ?1",
                params![tenant_id],
                |r| r.get(0),
            )?;
            if held >= max {
                return Err(AppError::Structured {
                    code: NOTIFY_CHANNEL_LIMIT,
                    message: format!("{held} channels are already registered; this plan allows {max} (notify_channels_max)"),
                    data: json!({"limit": max, "used": held, "notify_channels_max": max,
                                 "next_call": "host.notify.channel_list"}),
                });
            }
            conn.execute(
                "INSERT INTO notify_channels (id, tenant_id, kind, target_enc, target_nonce, target_hint, created_unix) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![row_id, tenant_id, kind.as_str(), target_enc, target_nonce, row_hint, now_unix()],
            )?;
            Ok(())
        })
        .await?;
    Ok(json!({"channel_id": id, "kind": kind.as_str(), "target_hint": hint}))
}

/// `host.notify.channel_list`.
pub async fn channel_list(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_args(state, args, "host.notify.channel_list")?;
    let tenant_id = tenant.id;
    let channels = state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, kind, target_hint, created_unix, disabled_unix FROM notify_channels \
                 WHERE tenant_id = ?1 ORDER BY created_unix, id",
            )?;
            let rows = stmt
                .query_map(params![tenant_id], |r| {
                    let disabled: Option<i64> = r.get(4)?;
                    let mut row = json!({
                        "channel_id": r.get::<_, String>(0)?,
                        "kind": r.get::<_, String>(1)?,
                        "target_hint": r.get::<_, String>(2)?,
                        "created_unix": r.get::<_, i64>(3)?,
                    });
                    if let Some(at) = disabled {
                        row["disabled"] = json!(true);
                        row["disabled_unix"] = json!(at);
                    }
                    Ok(row)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(json!({"channels": channels}))
}

/// `host.notify.channel_remove {channel_id}`.
pub async fn channel_remove(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_args(state, args, "host.notify.channel_remove")?;
    let channel_id = arg_str(args, "channel_id")?;
    let (tenant_id, id) = (tenant.id, channel_id.clone());
    let removed = state
        .db
        .with_conn(move |conn| {
            Ok(conn.execute(
                "DELETE FROM notify_channels WHERE id = ?1 AND tenant_id = ?2",
                params![id, tenant_id],
            )? > 0)
        })
        .await?;
    if !removed {
        return Err(AppError::Structured {
            code: NOTIFY_CHANNEL_NOT_FOUND,
            message: format!("no channel '{channel_id}' on this tenant"),
            data: json!({"next_call": "host.notify.channel_list"}),
        });
    }
    Ok(json!({"channel_id": channel_id, "removed": true}))
}

/// One live channel read for a send.
struct LiveChannel {
    id: String,
    kind: ChannelKind,
    target_enc: Vec<u8>,
    target_nonce: Vec<u8>,
}

/// `host.notify.send {text, level, link?}`: validates, enforces the daily
/// limit, writes the notify and one queued receipt per live channel, spawns
/// delivery, and returns -- nothing on this path waits on the network.
pub async fn send(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_args(state, args, "host.notify.send")?;
    let text = arg_str(args, "text")?;
    if text.trim().is_empty() {
        return Err(AppError::InvalidArgs("text: must be non-empty".to_string()));
    }
    if text.len() > TEXT_MAX_BYTES {
        return Err(AppError::InvalidArgs(format!("text: at most {TEXT_MAX_BYTES} bytes, got {}", text.len())));
    }
    let level_name = arg_str(args, "level")?;
    let level = Level::parse(&level_name).ok_or_else(|| level_unknown(&level_name))?;
    let link = match args.get("link") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let parsed = reqwest::Url::parse(s).map_err(|e| AppError::InvalidArgs(format!("link: not a valid URL: {e}")))?;
            if parsed.scheme() != "https" {
                return Err(AppError::InvalidArgs("link: must be an https URL".to_string()));
            }
            let own = parsed.host_str().is_some_and(|h| h == "mcphost.dev" || h.ends_with(".mcphost.dev"));
            if !own {
                tracing::info!(tenant = %tenant.namespace, host = ?parsed.host_str(), "notify link names a host outside mcphost.dev");
            }
            Some(s.clone())
        }
        Some(_) => return Err(AppError::InvalidArgs("link: must be a string".to_string())),
    };
    let per_day = plan_for(state, tenant)?.notifies_per_day;

    let notify_id = new_ulid().to_ascii_lowercase();
    let now = now_unix();
    let (tenant_id, nid, row_text, row_link) = (tenant.id, notify_id.clone(), text.clone(), link.clone());
    let channels: Vec<LiveChannel> = state
        .db
        .with_conn(move |conn| {
            let (used, oldest): (i64, Option<i64>) = conn.query_row(
                "SELECT COUNT(*), MIN(created_unix) FROM notifies WHERE tenant_id = ?1 AND created_unix > ?2",
                params![tenant_id, now - DAY_SECS],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if used >= per_day {
                return Err(AppError::Structured {
                    code: NOTIFY_LIMIT,
                    message: format!("{used} notifies in the last day; this plan allows {per_day} (notifies_per_day)"),
                    data: json!({"limit": per_day, "used": used, "notifies_per_day": per_day,
                                 "resets_unix": oldest.unwrap_or(now) + DAY_SECS}),
                });
            }
            let mut stmt = conn.prepare(
                "SELECT id, kind, target_enc, target_nonce FROM notify_channels \
                 WHERE tenant_id = ?1 AND disabled_unix IS NULL ORDER BY created_unix, id",
            )?;
            let live: Vec<LiveChannel> = stmt
                .query_map(params![tenant_id], |r| {
                    let kind: String = r.get(1)?;
                    Ok((r.get::<_, String>(0)?, kind, r.get::<_, Vec<u8>>(2)?, r.get::<_, Vec<u8>>(3)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter_map(|(id, kind, target_enc, target_nonce)| {
                    ChannelKind::parse(&kind).map(|kind| LiveChannel { id, kind, target_enc, target_nonce })
                })
                .collect();
            if live.is_empty() {
                return Err(AppError::Structured {
                    code: NO_CHANNEL,
                    message: "no notify channel is registered; add one first".to_string(),
                    data: json!({"next_call": "host.notify.channel_add"}),
                });
            }
            conn.execute(
                "INSERT INTO notifies (id, tenant_id, text, level, link, created_unix) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![nid, tenant_id, row_text, level.as_str(), row_link, now],
            )?;
            for ch in &live {
                conn.execute(
                    "INSERT INTO notify_receipts (notify_id, tenant_id, channel_id, kind, status, created_unix) \
                     VALUES (?1, ?2, ?3, ?4, 'queued', ?5)",
                    params![nid, tenant_id, ch.id, ch.kind.as_str(), now],
                )?;
            }
            Ok(live)
        })
        .await?;

    let count = channels.len();
    let envelope = Envelope {
        tenant_display: tenant.display_name.clone(),
        text,
        level,
        link,
        notify_id: notify_id.clone(),
        unix: now,
    };
    for ch in channels {
        let (deliver_state, envelope, tenant_id) = (state.clone(), envelope.clone(), tenant.id);
        tokio::spawn(async move {
            deliver(&deliver_state, tenant_id, ch, envelope).await;
        });
    }
    Ok(json!({"notify_id": notify_id, "channels": count}))
}

/// What a delivery carries. Never a tenant key: the fields are the display
/// name, the message, and ids.
#[derive(Clone)]
struct Envelope {
    tenant_display: String,
    text: String,
    level: Level,
    link: Option<String>,
    notify_id: String,
    unix: i64,
}

impl Envelope {
    fn webhook_json(&self) -> Value {
        json!({
            "tenant_display": self.tenant_display,
            "text": self.text,
            "level": self.level.as_str(),
            "link": self.link,
            "notify_id": self.notify_id,
            "unix": self.unix,
        })
    }

    fn slack_json(&self) -> Value {
        let text = match &self.link {
            Some(link) => format!("{} {}", self.text, link),
            None => self.text.clone(),
        };
        json!({"text": text})
    }

    fn email(&self, to: String, from: String) -> crate::email::EmailMessage {
        let mut body = format!("{}\n", self.text);
        if let Some(link) = &self.link {
            body.push_str(&format!("\n{link}\n"));
        }
        body.push_str(&format!("\n-- sent by {} through mcphost ({})\n", self.tenant_display, self.level.as_str()));
        crate::email::EmailMessage {
            reply_to: crate::email::reply_to_address(&from),
            to,
            from,
            subject: format!("[{}] {}", self.level.as_str(), self.text.chars().take(60).collect::<String>()),
            text_body: body,
        }
    }
}

fn delivery_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(DELIVERY_TIMEOUT)
            .build()
            .expect("build notify delivery client")
    })
}

/// One attempt through `kind`'s transport; `Ok(upstream_status)` on a 2xx
/// (or an accepted email, status 200), `Err(status)` otherwise.
async fn attempt(
    state: &AppState,
    transport: Transport,
    target: &str,
    envelope: &Envelope,
) -> Result<i64, Option<i64>> {
    match transport {
        Transport::EmailClient => {
            let from = state.email_config.from.clone().unwrap_or_default();
            match state.email_client.send(&envelope.email(target.to_string(), from)).await {
                Ok(()) => Ok(200),
                Err(e) => Err(e.claim_email_status().map(i64::from)),
            }
        }
        Transport::JsonPost | Transport::SlackPost => {
            if url_allowed(state, target).await.is_err() {
                return Err(None);
            }
            let body = if transport == Transport::SlackPost { envelope.slack_json() } else { envelope.webhook_json() };
            match delivery_client().post(target).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => Ok(i64::from(resp.status().as_u16())),
                Ok(resp) => Err(Some(i64::from(resp.status().as_u16()))),
                Err(_) => Err(None),
            }
        }
    }
}

/// Background delivery of one (notify, channel): up to [`ATTEMPTS_MAX`]
/// attempts, the receipt updated after each.
async fn deliver(state: &AppState, tenant_id: i64, ch: LiveChannel, envelope: Envelope) {
    let target = match state.secrets.decrypt(&ch.target_enc, &ch.target_nonce) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, channel = %ch.id, "notify target undecryptable");
            let update = ReceiptUpdate { status: "failed", upstream: None, attempts: 0, delivered_unix: None };
            write_receipt(state, &envelope.notify_id, &ch.id, update).await;
            return;
        }
    };
    let transport = route(ch.kind).1;
    let backoff = retry_backoff();
    let mut last: Option<i64> = None;
    for n in 0..ATTEMPTS_MAX {
        if n > 0 {
            tokio::time::sleep(backoff[(n - 1).min(backoff.len() - 1)]).await;
        }
        match attempt(state, transport, &target, &envelope).await {
            Ok(status) => {
                let update = ReceiptUpdate {
                    status: "delivered",
                    upstream: Some(status),
                    attempts: (n + 1) as i64,
                    delivered_unix: Some(now_unix()),
                };
                write_receipt(state, &envelope.notify_id, &ch.id, update).await;
                note_outcome(state, tenant_id, &ch.id, true).await;
                return;
            }
            Err(status) => {
                last = status;
                let update = ReceiptUpdate { status: "queued", upstream: last, attempts: (n + 1) as i64, delivered_unix: None };
                write_receipt(state, &envelope.notify_id, &ch.id, update).await;
            }
        }
    }
    let update = ReceiptUpdate { status: "failed", upstream: last, attempts: ATTEMPTS_MAX as i64, delivered_unix: None };
    write_receipt(state, &envelope.notify_id, &ch.id, update).await;
    note_outcome(state, tenant_id, &ch.id, false).await;
}

/// The mutable columns of one receipt row.
struct ReceiptUpdate {
    status: &'static str,
    upstream: Option<i64>,
    attempts: i64,
    delivered_unix: Option<i64>,
}

async fn write_receipt(state: &AppState, notify_id: &str, channel_id: &str, update: ReceiptUpdate) {
    let ReceiptUpdate { status, upstream, attempts, delivered_unix } = update;
    let (nid, cid) = (notify_id.to_string(), channel_id.to_string());
    let result = state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE notify_receipts SET status = ?3, upstream_status = ?4, attempts = ?5, delivered_unix = ?6 \
                 WHERE notify_id = ?1 AND channel_id = ?2",
                params![nid, cid, status, upstream, attempts, delivered_unix],
            )?;
            Ok(())
        })
        .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "notify receipt write failed");
    }
}

/// Tracks consecutive failures on the channel: reset by a delivery, and the
/// [`DISABLE_AFTER_FAILURES`]th failure in a row disables it.
async fn note_outcome(state: &AppState, tenant_id: i64, channel_id: &str, delivered: bool) {
    let cid = channel_id.to_string();
    let result = state
        .db
        .with_conn(move |conn| {
            if delivered {
                conn.execute(
                    "UPDATE notify_channels SET consecutive_failures = 0 WHERE id = ?1 AND tenant_id = ?2",
                    params![cid, tenant_id],
                )?;
            } else {
                conn.execute(
                    "UPDATE notify_channels SET consecutive_failures = consecutive_failures + 1, \
                     disabled_unix = CASE WHEN consecutive_failures + 1 >= ?3 THEN ?4 ELSE disabled_unix END \
                     WHERE id = ?1 AND tenant_id = ?2",
                    params![cid, tenant_id, DISABLE_AFTER_FAILURES, now_unix()],
                )?;
            }
            Ok(())
        })
        .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, "notify channel outcome write failed");
    }
}

/// `host.notify.receipts {notify_id?, since_unix?}`: newest first, at most
/// [`RECEIPTS_MAX`] rows.
pub async fn receipts(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_args(state, args, "host.notify.receipts")?;
    let notify_id = args.get("notify_id").and_then(Value::as_str).map(str::to_string);
    let since = args.get("since_unix").and_then(Value::as_i64).unwrap_or(0);
    let tenant_id = tenant.id;
    let rows = state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT notify_id, channel_id, kind, status, upstream_status, attempts, created_unix, delivered_unix \
                 FROM notify_receipts WHERE tenant_id = ?1 AND (?2 IS NULL OR notify_id = ?2) AND created_unix >= ?3 \
                 ORDER BY id DESC LIMIT ?4",
            )?;
            let rows = stmt
                .query_map(params![tenant_id, notify_id, since, RECEIPTS_MAX], |r| {
                    Ok(json!({
                        "notify_id": r.get::<_, String>(0)?,
                        "channel_id": r.get::<_, String>(1)?,
                        "kind": r.get::<_, String>(2)?,
                        "status": r.get::<_, String>(3)?,
                        "upstream_status": r.get::<_, Option<i64>>(4)?,
                        "attempts": r.get::<_, i64>(5)?,
                        "created_unix": r.get::<_, i64>(6)?,
                        "delivered_unix": r.get::<_, Option<i64>>(7)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;
    Ok(json!({"receipts": rows}))
}

/// The most receipt rows one `host.notify.receipts` call returns.
pub const RECEIPTS_MAX: i64 = 100;

crate::err_code! {
    pub EMAIL_NOT_OWNER,
    code: "email_not_owner", family: Args, class: AgentFault, retryable: false,
    remedy: "Set target to the owner's verified email address, or add a webhook or slack channel for another destination.",
}

crate::err_code! {
    pub NO_CHANNEL,
    code: "no_channel", family: Args, class: AgentFault, retryable: false,
    remedy: "Register a channel with host.notify.channel_add, then send the notify again.",
}

crate::err_code! {
    pub NOTIFY_CHANNEL_LIMIT,
    code: "notify_channel_limit", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Remove a channel with host.notify.channel_remove or move to a plan with a higher notify_channels_max.",
}

crate::err_code! {
    pub NOTIFY_CHANNEL_NOT_FOUND,
    code: "notify_channel_not_found", family: Args, class: AgentFault, retryable: false,
    remedy: "List channels with host.notify.channel_list and use a channel_id that exists on this tenant.",
}

crate::err_code! {
    pub NOTIFY_LIMIT,
    code: "notify_limit", family: Plan, class: PlanLimit, retryable: true,
    remedy: "The daily notify quota is used; retry after data.resets_unix or move to a plan with a higher notifies_per_day.",
}

crate::err_code! {
    pub NOTIFY_TARGET_INVALID,
    code: "notify_target_invalid", family: Args, class: AgentFault, retryable: false,
    remedy: "Fix target for the channel kind; a webhook or slack target must be an https URL on a public host.",
}

crate::err_code! {
    pub OWNER_UNVERIFIED,
    code: "owner_unverified", family: Auth, class: HostRefusal, retryable: false,
    remedy: "Verify the owner email first (data.next_call is host.claim), or add a webhook or slack channel instead.",
}

