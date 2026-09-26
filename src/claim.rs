//! PRD-mcphost-human-claim-magic-link: the human behind the agent claims
//! the namespace by email. `signup` hands back a `claim_url`
//! (requirement 1); this module owns everything downstream of that link --
//! `GET`/`POST /claim/{token}` and `GET /claim/verify/{code}` (wired in
//! `http.rs`), the claim/verify token lifecycle, and the summary page.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Form;
use serde::Deserialize;

use crate::auth::{generate_key, hash_key};
use crate::db::{ClaimVerifyOutcome, Tenant};
use crate::email::EmailMessage;
use crate::errors::AppError;
use crate::state::AppState;

/// AC1: a claim token is the same shape/entropy as a tenant key (32 random
/// bytes, hex) -- distinct only in what it's stored against
/// (`tenants.claim_token_hash`, never `tenants.key_hash`), so "not the
/// bearer key" (AC1) is true by construction: it's a wholly separate
/// random value, never derived from or equal to one.
pub fn generate_claim_token() -> String {
    generate_key()
}

/// requirement 3: the magic-link verify code -- same entropy/shape as a
/// claim token, stored (hashed) in its own `claim_codes` row instead of on
/// the tenant.
pub fn generate_verify_code() -> String {
    generate_key()
}

/// AC1's `claim_url` shape: `https://<host>/claim/<token>`, absolute.
/// Unlike `signup`'s `endpoint` (which just appends to `state.public_url`
/// verbatim), this always forces the `https://` scheme regardless of
/// `public_url`'s own -- a magic link handed to a human's inbox/browser
/// must never downgrade to plain HTTP even if `public_url` is configured
/// with `http://` for some internal reason (a bare IP:port in a test
/// harness, a loopback dev server); the host/port themselves still come
/// from `public_url`.
pub fn claim_url(public_url: &str, token: &str) -> String {
    let host = public_url
        .trim_end_matches('/')
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    format!("https://{host}/claim/{token}")
}

/// requirement 1: mints this tenant's single active claim token, stores
/// only its hash (`Db::set_claim_token`), and returns the absolute
/// `claim_url` `signup` puts in its response. Called exactly once, right
/// after `Db::create_tenant_attributed` -- same "issue right after the row
/// exists" timing `control::signup`'s handoff-token branch already uses.
pub async fn issue_claim_token(state: &AppState, tenant: &Tenant) -> Result<String, AppError> {
    let token = generate_claim_token();
    let token_hash = hash_key(&token);
    let expires_unix = crate::state::now_unix() + state.claim_token_ttl_secs;
    state
        .db
        .set_claim_token(tenant.id, token_hash, expires_unix)
        .await?;
    Ok(claim_url(&state.public_url, &token))
}

// ---- email validation --------------------------------------------------

/// AC5: "syntactically valid" -- no regex dependency for one shape check:
/// exactly one `@`, a non-empty local part, a domain part containing a
/// `.` that neither starts nor ends with one, and no whitespace anywhere
/// (a pasted-with-a-trailing-newline value is the realistic malformed
/// case this guards, not RFC 5322 edge cases).
pub fn is_valid_email(email: &str) -> bool {
    if email.is_empty() || email.len() > 254 || email.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = email.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

// ---- rendering -----------------------------------------------------------

fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const STYLE: &str = "body{margin:0;background:#0b0d0e;color:#c9cdd1;\
    font-family:ui-monospace,Menlo,Consolas,monospace;font-size:15px;line-height:1.65}\
    .wrap{max-width:34rem;margin:0 auto;padding:3rem 1.4rem}\
    h1{font-size:1.1rem;color:#e6edf3;margin:0 0 1rem}\
    p{margin:0 0 1rem}code{color:#e6edf3;background:#161b22;padding:.1em .35em;border-radius:4px}\
    input{background:#0d1117;border:1px solid #21262d;color:#e6edf3;padding:.5rem;width:100%;\
    font:inherit;border-radius:4px;box-sizing:border-box}\
    button{background:none;border:1px solid #21262d;border-radius:6px;padding:.5rem 1rem;\
    margin-top:.8rem;font:inherit;color:#e6edf3;cursor:pointer}\
    .err{color:#f85149}ul{padding-left:1.2rem}";

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title><style>{STYLE}</style></head>\
         <body><main class=\"wrap\">{body}</main></body></html>"
    )
}

fn html_response(status: StatusCode, body: String) -> Response {
    (status, [("content-type", "text/html; charset=utf-8")], body).into_response()
}

fn render_storage_error() -> String {
    page("mcphost — storage error", "<h1>Something went wrong</h1><p>Please try again in a minute.</p>")
}

/// AC4: the literal phrase the acceptance criterion names.
fn render_expired() -> String {
    page(
        "mcphost — link expired",
        "<h1>This link expired</h1><p>Ask your agent to sign up again, or ask the operator for a fresh claim link.</p>",
    )
}

/// AC3's second-open case: the code was already consumed.
fn render_verify_gone() -> String {
    page(
        "mcphost — link already used",
        "<h1>This link has already been used</h1><p>If you already finished claiming this tenant, you're done -- no further action needed.</p>",
    )
}

fn render_conflict() -> String {
    page(
        "mcphost — already claimed",
        "<h1>Someone else just claimed this tenant</h1><p>Another verification for this tenant completed first. If that wasn't you, contact the operator.</p>",
    )
}

/// AC6: the literal phrase the acceptance criterion names.
fn render_email_not_configured() -> String {
    page(
        "mcphost — claim your tenant",
        "<h1>Claim your tenant</h1><p>Sorry, email delivery is not configured on this host yet. \
         Ask the operator to set MCPHOST_EMAIL_API_URL, then reload this page.</p>",
    )
}

fn render_claim_form(token: &str, error: Option<&str>) -> String {
    let error_html = error
        .map(|e| format!("<p class=\"err\">{}</p>", html_escape(e)))
        .unwrap_or_default();
    page(
        "mcphost — claim your tenant",
        &format!(
            "<h1>Claim your tenant</h1>\
             <p>Enter your email and we'll send a link to verify you own this tenant.</p>\
             {error_html}\
             <form method=\"post\" action=\"/claim/{token}\">\
             <input type=\"email\" name=\"email\" placeholder=\"you@example.com\" required>\
             <button type=\"submit\">Send verification link</button>\
             </form>"
        ),
    )
}

/// AC2/AC10: the literal phrase the acceptance criteria name.
fn render_check_inbox() -> String {
    page(
        "mcphost — check your inbox",
        "<h1>Check your inbox</h1><p>We sent a verification link to your email. It expires in 30 minutes.</p>",
    )
}

/// requirement 7 / AC8: the literal 429 page every rate-limited
/// `GET`/`POST /claim/*` gets.
fn render_rate_limited() -> String {
    page(
        "mcphost — too many requests",
        "<h1>Too many requests</h1><p>Try again later.</p>",
    )
}

/// PRD-mcphost-abuse-guard-ban-list requirement 2 / AC5: every `/claim/*`
/// route's refusal for a banned address -- `reason` is never rendered here
/// even when the ban is `public: true` (the HTML page is not the JSON-RPC
/// `data` object AC3 governs; keeping this page fixed avoids a second
/// place that could leak an operator's free-text reason to a browser).
fn render_banned() -> String {
    page(
        "mcphost — banned",
        "<h1>This address is banned</h1><p>Contact the operator if you believe this is a mistake.</p>",
    )
}

fn render_send_error(token: &str) -> String {
    page(
        "mcphost — could not send",
        &format!(
            "<h1>We could not send the email, try again in a minute</h1>\
             <p><a href=\"/claim/{token}\">Try again</a></p>"
        ),
    )
}

/// requirement 4 / AC3: namespace, tool count and names, schedules and
/// webhooks (count, next fire), calls today vs plan limit, state bytes vs
/// quota, plan name, and an upgrade note -- everything is best-effort
/// (`unwrap_or_default`/`unwrap_or(0)`): a summary field failing to load
/// must never turn a successful claim into an error page.
async fn render_summary(state: &AppState, tenant: &Tenant, owner_email: &str) -> String {
    let tools = state.db.list_tools(tenant.id).await.unwrap_or_default();
    let triggers = state
        .db
        .list_triggers(tenant.id, None)
        .await
        .unwrap_or_default();
    let schedule_count = triggers.iter().filter(|t| t.kind == "schedule").count();
    let webhook_count = triggers.iter().filter(|t| t.kind == "webhook").count();
    let next_fire = triggers
        .iter()
        .filter(|t| t.kind == "schedule")
        .filter_map(|t| t.next_unix)
        .min();
    let calls_today = state
        .db
        .count_calls_since(
            tenant.id,
            crate::state::utc_midnight_unix(crate::state::now_unix()),
            true,
        )
        .await
        .unwrap_or(0);
    let state_bytes = state.db.state_bytes_used(tenant.id).await.unwrap_or(0);
    let plan = state.plans.get(&tenant.plan);

    let tool_names_html = if tools.is_empty() {
        "<li>(none published yet)</li>".to_string()
    } else {
        tools
            .iter()
            .map(|t| format!("<li><code>{}</code></li>", html_escape(&t.name)))
            .collect::<Vec<_>>()
            .join("")
    };
    let next_fire_html = next_fire
        .map(|u| format!(" (next fire: {})", crate::state::rfc3339_from_unix(u)))
        .unwrap_or_default();
    let (calls_limit, state_bytes_max) = plan
        .map(|p| (p.calls_per_day, p.state_bytes_max))
        .unwrap_or((0, 0));

    page(
        "mcphost — you're in",
        &format!(
            "<h1>You've claimed {namespace}</h1>\
             <p>Verified as <code>{email}</code>.</p>\
             <p>Plan: <code>{plan}</code></p>\
             <p>Tools published ({tool_count}):</p><ul>{tool_names_html}</ul>\
             <p>Schedules: {schedule_count}{next_fire_html}</p>\
             <p>Webhooks: {webhook_count}</p>\
             <p>Calls today: {calls_today} / {calls_limit}</p>\
             <p>State used: {state_bytes} / {state_bytes_max} bytes</p>\
             <p>Want more headroom? Ask your agent to call \
             <code>billing.checkout({{\"plan\": \"pro\", \"customer_email\": \"{email}\"}})</code> \
             to upgrade to Pro.</p>",
            namespace = html_escape(&tenant.namespace),
            email = html_escape(owner_email),
            plan = html_escape(&tenant.plan),
            tool_count = tools.len(),
        ),
    )
}

// ---- outbound email --------------------------------------------------------

/// technical considerations: "runs in a spawned task with one retry" --
/// literally spawned (not just called inline), but awaited before this
/// function returns, so `post_claim`'s response reflects the outcome
/// rather than optimistically claiming success before the provider has
/// even been asked.
async fn send_with_retry(state: &AppState, message: EmailMessage) -> Result<(), AppError> {
    let client = state.email_client.clone();
    let first = {
        let client = client.clone();
        let message = message.clone();
        tokio::spawn(async move { client.send(&message).await })
    };
    match first.await {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(e)) => tracing::warn!(error = %e, "claim email send failed, retrying once"),
        Err(e) => tracing::warn!(error = %e, "claim email send task panicked, retrying once"),
    }
    let retry = tokio::spawn(async move { client.send(&message).await });
    match retry.await {
        Ok(result) => result,
        Err(_) => Err(AppError::Internal("claim email retry task panicked".to_string())),
    }
}

/// requirement 2/3/7: mints a verify code (rate-limited to
/// [`crate::state::CLAIM_EMAIL_SEND_LIMIT_PER_HOUR`] sends per tenant per
/// hour), and sends the magic link through [`send_with_retry`].
async fn send_verify_email(state: &AppState, tenant: &Tenant, email: &str) -> Result<(), AppError> {
    let code = generate_verify_code();
    let code_hash = hash_key(&code);
    let now = crate::state::now_unix();
    let expires_unix = now + crate::state::CLAIM_VERIFY_CODE_TTL_SECS;
    let since_unix = now - 3600;
    let admitted = state
        .db
        .try_create_claim_code(
            tenant.id,
            code_hash,
            email.to_string(),
            expires_unix,
            since_unix,
            crate::state::CLAIM_EMAIL_SEND_LIMIT_PER_HOUR,
        )
        .await?;
    if !admitted {
        return Err(AppError::RateLimited);
    }
    let verify_url = {
        let host = state
            .public_url
            .trim_end_matches('/')
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        format!("https://{host}/claim/verify/{code}")
    };
    let message = EmailMessage {
        to: email.to_string(),
        subject: "Claim your mcphost tenant".to_string(),
        text_body: format!(
            "Click to verify your email and finish claiming your mcphost tenant:\n\n{verify_url}\n\n\
             This link expires in 30 minutes. If you didn't request this, ignore it."
        ),
    };
    send_with_retry(state, message).await
}

// ---- HTTP routes -----------------------------------------------------------

/// Same header-then-peer-fallback shape as `handler.rs`'s own
/// `source_ip` -- these routes sit on the same axum `Router` behind the
/// same reverse proxy, so they need the same `X-Forwarded-For` handling to
/// get a real per-caller address instead of Caddy's own loopback one.
fn source_ip(headers: &HeaderMap, peer: SocketAddr) -> String {
    let forwarded_for = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok());
    crate::state::resolve_source_ip(Some(&peer.ip().to_string()), forwarded_for)
}

/// requirement 7 / AC8: shared by every `/claim/*` route -- `None` on
/// success, the 429 page (or a 500 on a storage error) otherwise.
async fn check_claim_rate_limit(state: &AppState, headers: &HeaderMap, peer: SocketAddr) -> Option<Response> {
    let ip = source_ip(headers, peer);
    let since = crate::state::now_unix() - 3600;
    match state
        .db
        .try_admit_claim_request(ip, since, state.claim_rate_limit_per_hour)
        .await
    {
        Ok(true) => None,
        Ok(false) => Some(html_response(StatusCode::TOO_MANY_REQUESTS, render_rate_limited())),
        Err(_) => Some(html_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            render_storage_error(),
        )),
    }
}

/// PRD-mcphost-abuse-guard-ban-list requirement 2 / AC5: shared by every
/// `/claim/*` route -- `None` on success, the 403 banned page otherwise.
/// Checked before [`check_claim_rate_limit`] at each call site: a banned
/// address shouldn't spend the rate-limit budget it's about to be refused
/// from anyway.
async fn check_addr_ban(state: &AppState, headers: &HeaderMap, peer: SocketAddr) -> Option<Response> {
    let ip = source_ip(headers, peer);
    match crate::bans::enforce(state, "addr", &ip).await {
        Ok(()) => None,
        Err(_) => Some(html_response(StatusCode::FORBIDDEN, render_banned())),
    }
}

/// Shared by [`get_claim`]/[`post_claim`]: resolves a claim token to its
/// tenant, or the 410 page every unknown/expired token gets alike (AC4;
/// not distinguishing "never existed" from "expired" avoids leaking which
/// one a guess was).
pub(crate) async fn resolve_claim_token(state: &AppState, token: &str) -> Result<Tenant, Box<Response>> {
    let token_hash = hash_key(token);
    let tenant = match state.db.find_tenant_by_claim_token_hash(token_hash).await {
        Ok(Some(tenant)) => tenant,
        Ok(None) => return Err(Box::new(html_response(StatusCode::GONE, render_expired()))),
        Err(_) => {
            return Err(Box::new(html_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                render_storage_error(),
            )));
        }
    };
    match tenant.claim_expires_at {
        Some(expires_at) if crate::state::now_unix() <= expires_at => Ok(tenant),
        _ => Err(Box::new(html_response(StatusCode::GONE, render_expired()))),
    }
}

/// `GET /claim/{token}` (requirement 2, AC4, AC6, AC8).
pub async fn get_claim(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    if let Some(response) = check_addr_ban(&state, &headers, peer).await {
        return response;
    }
    if let Some(response) = check_claim_rate_limit(&state, &headers, peer).await {
        return response;
    }
    if let Err(response) = resolve_claim_token(&state, &token).await {
        return *response;
    }
    if !state.email_config.is_configured() {
        return html_response(StatusCode::OK, render_email_not_configured());
    }
    html_response(StatusCode::OK, render_claim_form(&token, None))
}

#[derive(Deserialize)]
pub struct ClaimForm {
    email: String,
}

/// `POST /claim/{token}` (requirement 2, AC2, AC5, AC8, AC10).
pub async fn post_claim(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Form(form): Form<ClaimForm>,
) -> Response {
    if let Some(response) = check_addr_ban(&state, &headers, peer).await {
        return response;
    }
    if let Some(response) = check_claim_rate_limit(&state, &headers, peer).await {
        return response;
    }
    let tenant = match resolve_claim_token(&state, &token).await {
        Ok(tenant) => tenant,
        Err(response) => return *response,
    };
    let email = form.email.trim();
    if !is_valid_email(email) {
        return html_response(
            StatusCode::BAD_REQUEST,
            render_claim_form(&token, Some("Enter a valid email address.")),
        );
    }
    // PRD-mcphost-abuse-guard-ban-list requirement 2: the claim flow's own
    // email_domain enforcement point -- checked once the address has
    // passed the shape check above, before a verify email is ever sent.
    if let Some(domain) = email.rsplit_once('@').map(|(_, d)| d)
        && crate::bans::enforce(&state, "email_domain", domain).await.is_err()
    {
        return html_response(StatusCode::FORBIDDEN, render_banned());
    }
    match send_verify_email(&state, &tenant, email).await {
        Ok(()) => html_response(StatusCode::OK, render_check_inbox()),
        Err(_) => html_response(StatusCode::OK, render_send_error(&token)),
    }
}

/// `GET /claim/verify/{code}` (requirement 3, AC3, AC7, AC8).
pub async fn get_verify(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    if let Some(response) = check_addr_ban(&state, &headers, peer).await {
        return response;
    }
    if let Some(response) = check_claim_rate_limit(&state, &headers, peer).await {
        return response;
    }
    let code_hash = hash_key(&code);
    match state.db.verify_claim_code(code_hash).await {
        Ok(ClaimVerifyOutcome::Verified { tenant_id, email }) => {
            match state.db.find_tenant_by_id(tenant_id).await {
                Ok(Some(tenant)) => {
                    html_response(StatusCode::OK, render_summary(&state, &tenant, &email).await)
                }
                _ => html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
            }
        }
        Ok(ClaimVerifyOutcome::Conflict) => html_response(StatusCode::CONFLICT, render_conflict()),
        Ok(
            ClaimVerifyOutcome::NotFound
            | ClaimVerifyOutcome::Expired
            | ClaimVerifyOutcome::AlreadyConsumed,
        ) => html_response(StatusCode::GONE, render_verify_gone()),
        Err(_) => html_response(StatusCode::INTERNAL_SERVER_ERROR, render_storage_error()),
    }
}
