//! PRD-mcphost-owner-verification-paths: three ways to prove "a human stands
//! behind this tenant" -- the email magic link (`claim.rs`), a GitHub OAuth
//! login, and a DNS TXT record -- all landing on the same single-winner
//! `UPDATE ... WHERE owner_verified_at IS NULL` ([`Db::claim_owner`] and
//! `Db::verify_claim_code`).
//!
//! [`VERIFICATION_METHODS`] is the one table: it generates the `method` enum
//! on every `host.claim.*` descriptor, `/claim/methods.json`, the
//! `host.quickstart` hint and the dispatch check; a drift test (ownv_ac02)
//! also pins the migration's CHECK list to it.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use axum::Json;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::hash_key;
use crate::claim::{html_escape, html_response, page};
use crate::db::{ClaimOwnerOutcome, Tenant};
use crate::errors::AppError;
use crate::state::AppState;

/// `(method, tool)`: the single table behind every verification surface.
pub const VERIFICATION_METHODS: &[(&str, &str)] = &[
    ("email", "host.claim.start"),
    ("github", "host.claim.github"),
    ("dns", "host.claim.dns"),
];

/// Tools that belong to the `host.claim.*` family but are not a method's
/// entry point: they take the same `method` enum, pinned to their parent.
pub const CLAIM_FOLLOWUP_TOOLS: &[(&str, &str)] = &[("host.claim.dns.check", "dns")];

/// Seconds a GitHub authorize state stays redeemable (R3).
pub const GITHUB_STATE_TTL_SECS: i64 = 600;
/// R4: the resolver deadline.
pub const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

pub fn method_names() -> Vec<&'static str> {
    VERIFICATION_METHODS.iter().map(|(m, _)| *m).collect()
}

/// The `method` property every `host.claim.*` descriptor carries.
pub fn method_property() -> Value {
    json!({
        "type": "string",
        "enum": method_names(),
        "description": "The verification method this tool performs; optional, and refused \
            when it names a different method than the tool does.",
    })
}

/// Every `host.claim.*` tool name: each method's entry point and the follow-ups.
pub fn claim_tool_names() -> Vec<&'static str> {
    VERIFICATION_METHODS
        .iter()
        .map(|(_, t)| *t)
        .chain(CLAIM_FOLLOWUP_TOOLS.iter().map(|(t, _)| *t))
        .collect()
}

/// The method a `host.claim.*` tool performs.
pub fn method_of_tool(tool: &str) -> Option<&'static str> {
    VERIFICATION_METHODS
        .iter()
        .find(|(_, t)| *t == tool)
        .map(|(m, _)| *m)
        .or_else(|| CLAIM_FOLLOWUP_TOOLS.iter().find(|(t, _)| *t == tool).map(|(_, m)| *m))
}

// ---- GitHub configuration -----------------------------------------------------

/// `MCPHOST_GITHUB_CLIENT_ID` / `MCPHOST_GITHUB_CLIENT_SECRET` (names only in
/// docs and logs). `MCPHOST_GITHUB_BASE_URL` and `MCPHOST_GITHUB_API_URL`
/// default to github.com / api.github.com and exist so a stub can stand in.
pub struct GithubConfig {
    client_id: String,
    client_secret: String,
    base_url: String,
    api_url: String,
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

pub fn github_config() -> Option<GithubConfig> {
    Some(GithubConfig {
        client_id: env_nonempty("MCPHOST_GITHUB_CLIENT_ID")?,
        client_secret: env_nonempty("MCPHOST_GITHUB_CLIENT_SECRET")?,
        base_url: env_nonempty("MCPHOST_GITHUB_BASE_URL").unwrap_or_else(|| "https://github.com".to_string()),
        api_url: env_nonempty("MCPHOST_GITHUB_API_URL").unwrap_or_else(|| "https://api.github.com".to_string()),
    })
}

fn method_available(state: &AppState, method: &str) -> bool {
    match method {
        "email" => state.email_config.is_configured(),
        "github" => github_config().is_some(),
        _ => true,
    }
}

/// `/claim/methods.json`: the table plus whether this host can run each method.
pub fn methods_json(state: &AppState) -> Value {
    let methods: Vec<Value> = VERIFICATION_METHODS
        .iter()
        .map(|(name, tool)| json!({"name": name, "tool": tool, "available": method_available(state, name)}))
        .collect();
    json!({"methods": methods})
}

pub async fn get_methods(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(methods_json(&state))
}

/// `host.quickstart.next`'s entry for an unverified tenant, from the table.
pub fn quickstart_next_step() -> Value {
    let methods: Vec<Value> = VERIFICATION_METHODS
        .iter()
        .map(|(method, tool)| json!({"method": method, "call": tool}))
        .collect();
    let names = method_names().join(", ");
    json!({
        "call": VERIFICATION_METHODS[0].1,
        "note": format!("Prove a human stands behind this tenant by one of: {names}."),
        "methods": methods,
    })
}

// ---- DNS resolver (injectable) ----------------------------------------------------

pub type TxtFuture = Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send>>;

/// TXT lookups; the production impl is `hickory-resolver`, tests install a
/// [`FixtureResolver`].
pub trait TxtResolver: Send + Sync {
    fn txt(&self, name: String) -> TxtFuture;
}

pub struct HickoryTxt(hickory_resolver::TokioResolver);

impl HickoryTxt {
    pub fn new() -> Result<Self, String> {
        let builder = hickory_resolver::Resolver::builder_tokio().map_err(|e| format!("dns resolver init: {e}"))?;
        Ok(Self(builder.build().map_err(|e| format!("dns resolver build: {e}"))?))
    }
}

impl TxtResolver for HickoryTxt {
    fn txt(&self, name: String) -> TxtFuture {
        let resolver = self.0.clone();
        Box::pin(async move {
            let lookup = resolver.txt_lookup(name).await.map_err(|e| e.to_string())?;
            Ok(lookup
                .answers()
                .iter()
                .filter_map(|record| match &record.data {
                    hickory_resolver::proto::rr::RData::TXT(txt) => Some(
                        txt.txt_data.iter().map(|part| String::from_utf8_lossy(part).into_owned()).collect::<String>(),
                    ),
                    _ => None,
                })
                .collect())
        })
    }
}

/// Test resolver: records keyed by the full record name.
#[derive(Default)]
pub struct FixtureResolver {
    records: Mutex<HashMap<String, Vec<String>>>,
}

impl FixtureResolver {
    pub fn set(&self, name: &str, values: Vec<String>) {
        if let Ok(mut records) = self.records.lock() {
            records.insert(name.to_string(), values);
        }
    }
}

impl TxtResolver for FixtureResolver {
    fn txt(&self, name: String) -> TxtFuture {
        let found = self.records.lock().ok().and_then(|r| r.get(&name).cloned());
        Box::pin(async move { found.ok_or_else(|| "NXDOMAIN".to_string()) })
    }
}

fn resolver_slot() -> &'static RwLock<Option<Arc<dyn TxtResolver>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn TxtResolver>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Installs `resolver` process-wide (the injection point R4 asks for).
pub fn install_resolver(resolver: Arc<dyn TxtResolver>) {
    if let Ok(mut slot) = resolver_slot().write() {
        *slot = Some(resolver);
    }
}

/// The shared [`FixtureResolver`], installed on first use; tests key their
/// records by their own domains, so parallel tests never collide.
pub fn fixture_resolver() -> Arc<FixtureResolver> {
    static FIXTURE: OnceLock<Arc<FixtureResolver>> = OnceLock::new();
    let fixture = FIXTURE.get_or_init(|| Arc::new(FixtureResolver::default())).clone();
    install_resolver(fixture.clone());
    fixture
}

fn current_resolver() -> Result<Arc<dyn TxtResolver>, AppError> {
    if let Some(r) = resolver_slot().read().ok().and_then(|slot| slot.clone()) {
        return Ok(r);
    }
    static DEFAULT: OnceLock<Result<Arc<dyn TxtResolver>, String>> = OnceLock::new();
    match DEFAULT.get_or_init(|| HickoryTxt::new().map(|r| Arc::new(r) as Arc<dyn TxtResolver>)) {
        Ok(r) => Ok(r.clone()),
        Err(e) => Err(AppError::Internal(e.clone())),
    }
}

// ---- shared tool plumbing ---------------------------------------------------------

fn already_verified(method: &str) -> AppError {
    AppError::Structured {
        code: "already_verified",
        message: format!("this tenant is already verified (method: {method}); nothing was changed"),
        data: json!({"method": method}),
    }
}

fn method_unavailable(method: &str, why: &str) -> AppError {
    AppError::Structured {
        code: "method_unavailable",
        message: format!("verification by {method} is not available on this host: {why}"),
        data: json!({"method": method}),
    }
}

/// R6: every path short-circuits on a verified tenant. Reads the live row,
/// not the `Tenant` the dispatch captured, so a claim that landed meanwhile counts.
async fn refuse_if_verified(state: &AppState, tenant: &Tenant) -> Result<(), AppError> {
    match state.db.owner_method(tenant.id).await? {
        Some((method, _)) => Err(already_verified(&method)),
        None => Ok(()),
    }
}

fn check_method_arg(tool: &str, args: &Value) -> Result<(), AppError> {
    let own = method_of_tool(tool).unwrap_or_default();
    match args.get("method").and_then(Value::as_str) {
        Some(m) if m != own => Err(AppError::InvalidArgs(format!("{tool} performs method '{own}', not '{m}'"))),
        _ => Ok(()),
    }
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, AppError> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::InvalidArgs(format!("'{name}' is required")))
}

// ---- host.claim.start (email) -----------------------------------------------------

pub async fn claim_start(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_method_arg("host.claim.start", args)?;
    refuse_if_verified(state, tenant).await?;
    if !state.email_config.is_configured() {
        return Err(method_unavailable("email", "outgoing mail is not configured"));
    }
    let email = str_arg(args, "email")?;
    if !crate::claim::is_valid_email(email) {
        return Err(AppError::InvalidArgs("'email' is not a valid address".to_string()));
    }
    if let Some(domain) = email.rsplit_once('@').map(|(_, d)| d) {
        crate::bans::enforce(state, "email_domain", domain).await?;
    }
    crate::claim::send_verify_email(state, tenant, email).await?;
    Ok(json!({
        "method": "email",
        "sent": true,
        "email_masked": crate::claim::mask_email(email),
        "expires_in": crate::state::CLAIM_VERIFY_CODE_TTL_SECS,
    }))
}

// ---- host.claim.github ------------------------------------------------------------

fn github_redirect_uri(state: &AppState) -> String {
    format!("{}/claim/github/callback", state.public_url.trim_end_matches('/'))
}

/// RFC 3986 unreserved characters pass through; everything else is escaped.
const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

fn enc(value: &str) -> String {
    utf8_percent_encode(value, QUERY_VALUE).to_string()
}

pub async fn claim_github(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_method_arg("host.claim.github", args)?;
    refuse_if_verified(state, tenant).await?;
    let Some(config) = github_config() else {
        return Err(method_unavailable("github", "MCPHOST_GITHUB_CLIENT_ID / MCPHOST_GITHUB_CLIENT_SECRET are not set"));
    };
    let nonce = crate::auth::generate_key();
    let expires_unix = crate::state::now_unix() + GITHUB_STATE_TTL_SECS;
    state.db.create_github_claim_state(tenant.id, hash_key(&nonce), expires_unix).await?;
    let authorize_url = format!(
        "{}/login/oauth/authorize?client_id={}&redirect_uri={}&state={}",
        config.base_url.trim_end_matches('/'),
        enc(&config.client_id),
        enc(&github_redirect_uri(state)),
        enc(&nonce),
    );
    Ok(json!({"method": "github", "authorize_url": authorize_url, "expires_in": GITHUB_STATE_TTL_SECS}))
}

#[derive(Deserialize)]
pub struct GithubCallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

fn render_github_error(title: &str, message: &str) -> String {
    page(
        &format!("mcphost — {title}"),
        &format!("<h1>{}</h1><p>{}</p>", html_escape(title), html_escape(message)),
    )
}

/// `GET /claim/github/callback`: CSRF by the state row, code exchange through
/// the host's egress, then the single-winner UPDATE.
pub async fn get_github_callback(
    State(state): State<Arc<AppState>>,
    Query(q): Query<GithubCallbackQuery>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let bad_link = || {
        html_response(
            StatusCode::BAD_REQUEST,
            render_github_error("That link has expired", "Ask your agent for a new GitHub link."),
        )
    };
    let storage = || html_response(StatusCode::INTERNAL_SERVER_ERROR, render_github_error("Something went wrong", "Please try again in a minute."));
    let (Some(code), Some(nonce)) = (q.code.filter(|c| !c.is_empty()), q.state.filter(|s| !s.is_empty())) else {
        return bad_link();
    };
    if let Some(response) = crate::claim::check_addr_ban(&state, &headers, peer).await {
        return response;
    }
    if let Some(response) = crate::claim::check_claim_rate_limit(&state, &headers, peer).await {
        return response;
    }
    let tenant_id = match state.db.take_github_claim_state(hash_key(&nonce)).await {
        Ok(Some(id)) => id,
        Ok(None) => return bad_link(),
        Err(_) => return storage(),
    };
    let Some(config) = github_config() else {
        return html_response(
            StatusCode::SERVICE_UNAVAILABLE,
            render_github_error("GitHub is not set up here", "This host cannot verify by GitHub yet."),
        );
    };
    let upstream = || html_response(StatusCode::BAD_GATEWAY, render_github_error("GitHub did not answer", "Ask your agent for a new link and try again."));
    let Some(identity) = fetch_github_identity(&state, &config, &code).await else {
        return upstream();
    };
    let detail = json!({"github_id": identity.id}).to_string();
    match state.db.claim_owner(tenant_id, "github", identity.login.clone(), Some(detail)).await {
        Ok(ClaimOwnerOutcome::Verified) => {
            let name = match state.db.find_tenant_by_id(tenant_id).await {
                Ok(Some(t)) => t.display_name,
                _ => String::new(),
            };
            html_response(
                StatusCode::OK,
                page(
                    "mcphost — verified",
                    &format!(
                        "<h1>{} is yours</h1><p>Verified as GitHub user <code>{}</code>. Your agent keeps working exactly as before. You can close this page.</p>",
                        html_escape(&name),
                        html_escape(&identity.login),
                    ),
                ),
            )
        }
        Ok(ClaimOwnerOutcome::Already { .. }) => html_response(
            StatusCode::CONFLICT,
            render_github_error("Someone else finished first", "This backend already has a verified owner."),
        ),
        Err(_) => storage(),
    }
}

struct GithubIdentity {
    login: String,
    id: i64,
}

async fn fetch_github_identity(state: &AppState, config: &GithubConfig, code: &str) -> Option<GithubIdentity> {
    let redirect_uri = github_redirect_uri(state);
    let token_resp = state
        .http_client
        .post(format!("{}/login/oauth/access_token", config.base_url.trim_end_matches('/')))
        .header("accept", "application/json")
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success())?;
    let body: Value = token_resp.json().await.ok()?;
    let access_token = body.get("access_token").and_then(Value::as_str)?.to_string();
    let user_resp = state
        .http_client
        .get(format!("{}/user", config.api_url.trim_end_matches('/')))
        .header("accept", "application/json")
        .header("user-agent", "mcphost")
        .bearer_auth(access_token)
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success())?;
    let user: Value = user_resp.json().await.ok()?;
    let login = user.get("login").and_then(Value::as_str).filter(|l| !l.is_empty())?.to_string();
    let id = user.get("id").and_then(Value::as_i64)?;
    Some(GithubIdentity { login, id })
}

// ---- host.claim.dns / host.claim.dns.check ----------------------------------------

/// Lower-cased, trailing-dot-free hostname of dot-separated LDH labels.
fn normalize_domain(raw: &str) -> Result<String, AppError> {
    let domain = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    if domain.len() > 253 || !domain.contains('.') || !domain.split('.').all(label_ok) {
        return Err(AppError::InvalidArgs("'domain' must be a hostname such as acme.dev".to_string()));
    }
    Ok(domain)
}

fn record_name(domain: &str) -> String {
    format!("_mcphost.{domain}")
}

fn record_value(token: &str) -> String {
    format!("mcphost-verify={token}")
}

pub async fn claim_dns(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_method_arg("host.claim.dns", args)?;
    refuse_if_verified(state, tenant).await?;
    let domain = normalize_domain(str_arg(args, "domain")?)?;
    let token = state.db.dns_claim_token(tenant.id, domain.clone(), crate::auth::generate_key()).await?;
    Ok(json!({
        "method": "dns",
        "record": record_name(&domain),
        "type": "TXT",
        "value": record_value(&token),
        "next": {"call": "host.claim.dns.check", "arguments": {"domain": domain}},
    }))
}

pub async fn claim_dns_check(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_method_arg("host.claim.dns.check", args)?;
    refuse_if_verified(state, tenant).await?;
    let domain = normalize_domain(str_arg(args, "domain")?)?;
    let Some(token) = state.db.find_dns_claim_token(tenant.id, domain.clone()).await? else {
        return Err(AppError::Structured {
            code: "dns_claim_not_started",
            message: format!("no DNS claim for {domain}; call host.claim.dns first"),
            data: json!({"next": "host.claim.dns", "domain": domain}),
        });
    };
    let expected = record_value(&token);
    let record = record_name(&domain);
    let seen = match tokio::time::timeout(DNS_LOOKUP_TIMEOUT, current_resolver()?.txt(record.clone())).await {
        Ok(Ok(values)) => values,
        Ok(Err(_)) | Err(_) => Vec::new(),
    };
    if !seen.iter().any(|v| v.trim() == expected) {
        return Err(AppError::Structured {
            code: "dns_record_not_found",
            message: format!("{record} has no TXT record equal to the expected value"),
            data: json!({"record": record, "expected": expected, "seen": seen}),
        });
    }
    match state.db.claim_owner(tenant.id, "dns", domain.clone(), None).await? {
        ClaimOwnerOutcome::Verified => Ok(json!({"verified": true, "method": "dns", "handle": domain})),
        ClaimOwnerOutcome::Already { method } => Err(already_verified(&method)),
    }
}
