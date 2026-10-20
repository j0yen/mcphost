//! PRD-mcphost-owner-verification-paths: three ways to prove "a human stands
//! behind this tenant" -- an email link (`claim.rs`), a GitHub login, or a DNS
//! TXT record -- all setting `owner_verified_at` through the same single-winner
//! `UPDATE ... WHERE owner_verified_at IS NULL` ([`crate::db::Db::verify_owner`]).
//!
//! [`VERIFICATION_METHODS`] is the one table: it generates the `method` enum
//! in every `host.claim.*` descriptor ([`tools`]), `/claim/methods.json`
//! ([`methods_json`]) and the quickstart `next` steps ([`next_steps`]); a
//! drift test (`ownv_ac02`) checks the handler arms, the migration CHECK and
//! the JSON against it.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rmcp::model::Tool;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::generate_key;
use crate::claim::{html_escape, html_response, page};
use crate::db::{OwnerVerifyOutcome, Tenant};
use crate::errors::AppError;
use crate::state::{AppState, now_unix};

/// `(method, the tool that starts it)`. The single source for the `method`
/// enum, `/claim/methods.json`, the quickstart steps, the migration CHECK and
/// the `tenant_audit` kinds ([`audit_kinds`]). `admin` is the operator path
/// (PRD-mcphost-email-send-live-proof R5): an agent cannot start it, so every
/// surface an agent reads lists only [`agent_startable`] entries.
pub const VERIFICATION_METHODS: &[(&str, &str)] = &[
    ("email", "host.claim.start"),
    ("github", "host.claim.github"),
    ("dns", "host.claim.dns.start"),
    ("admin", "admin.tenant_verify_owner"),
];

/// The table entries an agent can start: the `host.*` tools.
pub fn agent_startable() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    VERIFICATION_METHODS.iter().filter(|(_, tool)| tool.starts_with("host."))
}

/// The `tenant_audit.action` a method's verification writes.
pub fn audit_kind(method: &str) -> String {
    format!("claim.{method}")
}

/// Every audit kind the table can write, in table order.
pub fn audit_kinds() -> Vec<String> {
    VERIFICATION_METHODS.iter().map(|(name, _)| audit_kind(name)).collect()
}

/// The tool that checks a published DNS record (a sibling of `host.claim.dns.start`).
pub const DNS_CHECK_TOOL: &str = "host.claim.dns.check";

/// How long a GitHub authorize `state` stays redeemable.
const GITHUB_STATE_TTL_SECS: i64 = 10 * 60;

/// How long `host.claim.dns.check` waits for the resolver.
const DNS_TIMEOUT: Duration = Duration::from_secs(5);

crate::err_code! {
    pub METHOD_UNAVAILABLE,
    code: "method_unavailable", family: Auth, class: HostRefusal, retryable: false,
    remedy: "This host has not configured that verification method; pick another from /claim/methods.json.",
}

crate::err_code! {
    pub ALREADY_VERIFIED,
    code: "already_verified", family: Auth, class: HostRefusal, retryable: false,
    remedy: "This tenant is already verified; nothing was written. The data names the method that verified it.",
}

crate::err_code! {
    pub OWNER_VERIFIED_BY_OTHER_METHOD,
    code: "owner_verified_by_other_method", family: Auth, class: HostRefusal, retryable: false,
    remedy: "This tenant's owner already proved themselves by another method; the admin path never overwrites a human's or a domain's proof. Nothing was written.",
}

crate::err_code! {
    pub DNS_RECORD_NOT_FOUND,
    code: "dns_record_not_found", family: Auth, class: AgentFault, retryable: true,
    remedy: "Publish the TXT record named in expected, wait for DNS to propagate, then call host.claim.dns.check again.",
}

/// Every method name, in table order.
pub fn method_names() -> Vec<&'static str> {
    VERIFICATION_METHODS.iter().map(|(name, _)| *name).collect()
}

/// Resolves the TXT strings published at a name (injectable for tests).
#[async_trait::async_trait]
pub trait DnsResolver: Send + Sync {
    async fn txt(&self, name: &str) -> Result<Vec<String>, String>;
}

/// Production [`DnsResolver`]: the host's `hickory-resolver`.
pub struct HickoryTxt;

#[async_trait::async_trait]
impl DnsResolver for HickoryTxt {
    async fn txt(&self, name: &str) -> Result<Vec<String>, String> {
        let resolver = hickory_resolver::Resolver::builder_tokio()
            .map_err(|e| e.to_string())?
            .build()
            .map_err(|e| e.to_string())?;
        let lookup = resolver.txt_lookup(format!("{name}.")).await.map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for record in lookup.answers() {
            if let hickory_resolver::proto::rr::RData::TXT(txt) = &record.data {
                let joined: Vec<u8> = txt.txt_data.iter().flat_map(|chunk| chunk.iter().copied()).collect();
                out.push(String::from_utf8_lossy(&joined).into_owned());
            }
        }
        Ok(out)
    }
}

/// The GitHub OAuth app, from `MCPHOST_GITHUB_CLIENT_ID` and
/// `MCPHOST_GITHUB_CLIENT_SECRET` (names only in docs and logs). The three
/// URLs default to github.com and exist as fields so a test can stub the token
/// endpoint.
#[derive(Clone)]
pub struct GithubConfig {
    pub client_id: String,
    pub client_secret: String,
    pub authorize_url: String,
    pub token_url: String,
    pub user_url: String,
}

impl GithubConfig {
    pub fn new(client_id: &str, client_secret: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            authorize_url: "https://github.com/login/oauth/authorize".to_string(),
            token_url: "https://github.com/login/oauth/access_token".to_string(),
            user_url: "https://api.github.com/user".to_string(),
        }
    }
}

/// Everything the non-email paths read from the environment, held on `AppState`.
#[derive(Clone)]
pub struct OwnerVerify {
    pub github: Option<GithubConfig>,
    pub resolver: Arc<dyn DnsResolver>,
}

impl Default for OwnerVerify {
    /// Nothing configured, real resolver: what a bare test state holds.
    fn default() -> Self {
        Self { github: None, resolver: Arc::new(HickoryTxt) }
    }
}

impl OwnerVerify {
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// `from_env` over an arbitrary variable lookup, so a test needs no `set_var`.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let github = match (get("MCPHOST_GITHUB_CLIENT_ID"), get("MCPHOST_GITHUB_CLIENT_SECRET")) {
            (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => {
                let mut cfg = GithubConfig::new(&id, &secret);
                if let Some(u) = get("MCPHOST_GITHUB_AUTHORIZE_URL") {
                    cfg.authorize_url = u;
                }
                if let Some(u) = get("MCPHOST_GITHUB_TOKEN_URL") {
                    cfg.token_url = u;
                }
                if let Some(u) = get("MCPHOST_GITHUB_USER_URL") {
                    cfg.user_url = u;
                }
                Some(cfg)
            }
            _ => None,
        };
        Self { github, resolver: Arc::new(HickoryTxt) }
    }

    fn available(&self, state: &AppState, method: &str) -> bool {
        match method {
            "email" => state.email_config.is_configured(),
            "github" => self.github.is_some(),
            _ => true,
        }
    }
}

/// `GET /claim/methods.json`: the method table plus whether this host can run each.
pub fn methods_json(state: &AppState) -> Value {
    let methods: Vec<Value> = agent_startable()
        .map(|(name, tool)| {
            json!({"name": name, "tool": tool, "available": state.owner_verify.available(state, name)})
        })
        .collect();
    json!({"methods": methods})
}

pub async fn methods_json_route(State(state): State<Arc<AppState>>) -> Response {
    axum::Json(methods_json(&state)).into_response()
}

/// The unverified-tenant `host.quickstart` steps, one per method, from the table.
pub fn next_steps() -> Vec<Value> {
    agent_startable()
        .map(|(name, tool)| {
            json!({
                "call": tool,
                "method": name,
                "note": format!("Verify ownership by {name}: see /claim/methods.json."),
            })
        })
        .collect()
}

// ---- descriptors ----------------------------------------------------------

/// The `host.claim.*` descriptors. Each carries the generated `method` enum.
pub(crate) fn tools() -> Vec<Tool> {
    let method_prop = |own: &str| {
        json!({
            "type": "string",
            "enum": method_names(),
            "description": format!("The verification method; this tool runs `{own}`. Optional."),
        })
    };
    // The example is a documentation-reserved name (RFC 2606) so a call generated from the
    // schema (the `verbfwd_ac08` stack-budget guard reads `examples`) reaches the verb body.
    let domain_prop = json!({
        "type": "string",
        "description": format!("The domain you control, e.g. \"{DOMAIN_EXAMPLE}\"."),
        "examples": [DOMAIN_EXAMPLE],
    });
    vec![
        Tool::new(
            "host.claim.start",
            "Verify ownership by email: returns the claim_url a human opens to receive a magic link. \
             Already-verified tenants get already_verified {method}.",
            crate::handler::host_schema(json!({"method": method_prop("email")}), &[]),
        ),
        Tool::new(
            "host.claim.github",
            "Verify ownership with GitHub: returns {authorize_url, expires_in}; the human opens it and the \
             authenticated GitHub login becomes the owner handle. method_unavailable when the host has no \
             GitHub app configured.",
            crate::handler::host_schema(json!({"method": method_prop("github")}), &[]),
        ),
        Tool::new(
            "host.claim.dns.start",
            "Verify ownership with a DNS record: returns the TXT record to publish at _mcphost.<domain>. \
             Then call host.claim.dns.check.",
            crate::handler::host_schema(
                json!({"domain": domain_prop.clone(), "method": method_prop("dns")}),
                &["domain"],
            ),
        ),
        Tool::new(
            DNS_CHECK_TOOL,
            "Look up the TXT record host.claim.dns.start asked for and, on a match, verify ownership. A mismatch \
             answers dns_record_not_found {expected, seen}.",
            crate::handler::host_schema(json!({"domain": domain_prop, "method": method_prop("dns")}), &["domain"]),
        ),
    ]
}

// ---- tool handlers --------------------------------------------------------

pub(crate) fn structured(code: &'static str, message: String, data: Value) -> AppError {
    AppError::Structured { code, message, data }
}

pub(crate) fn already_verified(method: &str) -> AppError {
    structured(
        ALREADY_VERIFIED,
        format!("this tenant is already verified by {method}"),
        json!({"method": method}),
    )
}

/// R6: every path answers `already_verified {method}` and writes nothing for a verified tenant.
async fn ensure_unverified(state: &AppState, tenant: &Tenant) -> Result<(), AppError> {
    if tenant.owner_verified_at.is_none() {
        return Ok(());
    }
    let (method, _) = state.db.owner_provenance(tenant.id).await?;
    Err(already_verified(method.as_deref().unwrap_or("email")))
}

/// `method`, when given, must name the method the tool runs.
fn check_method_arg(args: &Value, own: &str) -> Result<(), AppError> {
    match args.get("method").and_then(Value::as_str) {
        Some(m) if m != own => Err(AppError::InvalidArgs(format!(
            "method '{m}' does not match this tool, which runs '{own}'"
        ))),
        _ => Ok(()),
    }
}

/// `host.claim.start`: the email path's claim link.
pub async fn claim_start(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_method_arg(args, "email")?;
    ensure_unverified(state, tenant).await?;
    let claim_url = match crate::claim::claim_url_for_tenant(&state.public_url, tenant) {
        Some(url) => url,
        None => crate::claim::issue_claim_token(state, tenant).await?,
    };
    Ok(json!({"method": "email", "claim_url": claim_url}))
}

/// `host.claim.github`: a state row and GitHub's authorize URL.
pub async fn claim_github(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_method_arg(args, "github")?;
    let Some(cfg) = &state.owner_verify.github else {
        return Err(structured(
            METHOD_UNAVAILABLE,
            "GitHub verification is not configured on this host".to_string(),
            json!({"method": "github"}),
        ));
    };
    ensure_unverified(state, tenant).await?;
    let oauth_state = generate_key();
    state
        .db
        .put_github_state(oauth_state.clone(), tenant.id, now_unix() + GITHUB_STATE_TTL_SECS)
        .await?;
    let enc = |s: &str| percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string();
    let sep = if cfg.authorize_url.contains('?') { '&' } else { '?' };
    let authorize_url = format!(
        "{}{sep}client_id={}&redirect_uri={}&state={}&allow_signup=false",
        cfg.authorize_url,
        enc(&cfg.client_id),
        enc(&callback_url(&state.public_url)),
        enc(&oauth_state),
    );
    Ok(json!({"method": "github", "authorize_url": authorize_url, "expires_in": GITHUB_STATE_TTL_SECS}))
}

fn callback_url(public_url: &str) -> String {
    format!("{}/claim/github/callback", public_url.trim_end_matches('/'))
}

/// The `domain` example the descriptors publish (description and `examples`); a test asserts
/// `normalize_domain` accepts it, so the schema cannot advertise a domain the verb refuses.
pub const DOMAIN_EXAMPLE: &str = "example.com";

/// A lowercase, dot-separated hostname of letters, digits and hyphens.
fn normalize_domain(raw: &str) -> Result<String, AppError> {
    let domain = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    if domain.len() > 253 || !domain.contains('.') || !domain.split('.').all(label_ok) {
        return Err(AppError::InvalidArgs(format!("domain '{raw}' is not a valid domain name")));
    }
    Ok(domain)
}

fn domain_arg(args: &Value) -> Result<String, AppError> {
    let raw = args
        .get("domain")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'domain'".to_string()))?;
    normalize_domain(raw)
}

fn record_value(token: &str) -> String {
    format!("mcphost-verify={token}")
}

/// `host.claim.dns.start`: the TXT record to publish.
pub async fn claim_dns(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_method_arg(args, "dns")?;
    let domain = domain_arg(args)?;
    ensure_unverified(state, tenant).await?;
    let token = state.db.dns_challenge_token(tenant.id, domain.clone(), generate_key()).await?;
    Ok(json!({
        "method": "dns",
        "record": format!("_mcphost.{domain}"),
        "type": "TXT",
        "value": record_value(&token),
        "next": {"call": DNS_CHECK_TOOL, "arguments": {"domain": domain}},
    }))
}

/// `host.claim.dns.check`: resolve the record and, on a match, verify.
pub async fn claim_dns_check(state: &AppState, tenant: &Tenant, args: &Value) -> Result<Value, AppError> {
    check_method_arg(args, "dns")?;
    let domain = domain_arg(args)?;
    ensure_unverified(state, tenant).await?;
    let token = state.db.dns_challenge_token(tenant.id, domain.clone(), generate_key()).await?;
    let expected = record_value(&token);
    let name = format!("_mcphost.{domain}");
    let seen = match tokio::time::timeout(DNS_TIMEOUT, state.owner_verify.resolver.txt(&name)).await {
        Ok(Ok(records)) => records,
        Ok(Err(_)) | Err(_) => Vec::new(),
    };
    if !seen.iter().any(|r| r.trim() == expected) {
        return Err(structured(
            DNS_RECORD_NOT_FOUND,
            format!("no TXT record at {name} carries the expected value"),
            json!({"expected": expected, "seen": seen}),
        ));
    }
    match state.db.verify_owner(tenant.id, "dns".to_string(), domain.clone()).await? {
        OwnerVerifyOutcome::Verified => Ok(json!({"verified": true, "method": "dns", "handle": domain})),
        OwnerVerifyOutcome::Already { method } => Err(already_verified(&method)),
    }
}

// ---- GitHub callback ------------------------------------------------------

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

fn result_page(status: StatusCode, title: &str, message: &str) -> Response {
    let body = format!("<h1>{}</h1><p>{}</p>", html_escape(title), html_escape(message));
    html_response(status, page(title, &body))
}

type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Exchanges `code` at the token endpoint, then reads `login` from the user endpoint.
fn github_login(state: Arc<AppState>, cfg: GithubConfig, code: String) -> BoxFut<Result<String, ()>> {
    Box::pin(async move {
        let redirect_uri = callback_url(&state.public_url);
        let token_resp = state
            .http_client
            .post(&cfg.token_url)
            .header("accept", "application/json")
            .form(&[
                ("client_id", cfg.client_id.as_str()),
                ("client_secret", cfg.client_secret.as_str()),
                ("code", code.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
            ])
            .send()
            .await
            .map_err(|_| ())?;
        if !token_resp.status().is_success() {
            return Err(());
        }
        let body: Value = token_resp.json().await.map_err(|_| ())?;
        let access_token = body.get("access_token").and_then(Value::as_str).ok_or(())?.to_string();
        let user_resp = state
            .http_client
            .get(&cfg.user_url)
            .header("accept", "application/json")
            .header("user-agent", "mcphost")
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| ())?;
        if !user_resp.status().is_success() {
            return Err(());
        }
        let user: Value = user_resp.json().await.map_err(|_| ())?;
        user.get("login").and_then(Value::as_str).map(str::to_string).ok_or(())
    })
}

/// `GET /claim/github/callback`: consumes the state row, exchanges the code and
/// runs the single-winner UPDATE with `method = github, handle = login`.
pub async fn github_callback(State(state): State<Arc<AppState>>, Query(q): Query<CallbackQuery>) -> Response {
    let (Some(code), Some(oauth_state)) = (q.code, q.state) else {
        return result_page(StatusCode::BAD_REQUEST, "Link not valid", "The GitHub callback is missing its code or state.");
    };
    let Some(cfg) = state.owner_verify.github.clone() else {
        return result_page(StatusCode::BAD_REQUEST, "Not available", "GitHub verification is not configured on this host.");
    };
    let tenant_id = match state.db.take_github_state(oauth_state).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return result_page(StatusCode::BAD_REQUEST, "Link expired", "This GitHub verification link is unknown or has expired. Start again from host.claim.github.");
        }
        Err(_) => return result_page(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong", "Please try again."),
    };
    let Ok(login) = github_login(state.clone(), cfg, code).await else {
        return result_page(StatusCode::BAD_GATEWAY, "GitHub did not confirm", "GitHub rejected the code. Start again from host.claim.github.");
    };
    match state.db.verify_owner(tenant_id, "github".to_string(), login.clone()).await {
        Ok(OwnerVerifyOutcome::Verified) => {
            result_page(StatusCode::OK, "Verified", &format!("This tenant is now verified as GitHub user {login}."))
        }
        Ok(OwnerVerifyOutcome::Already { method }) => {
            result_page(StatusCode::OK, "Already verified", &format!("This tenant was already verified by {method}."))
        }
        Err(_) => result_page(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong", "Please try again."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema example (`DOMAIN_EXAMPLE`) is the one value both the descriptors and the
    /// verb body are checked against, so the two cannot drift apart.
    #[test]
    fn published_domain_example_passes_the_verbs_own_validation() {
        assert_eq!(normalize_domain(DOMAIN_EXAMPLE).expect("example is a valid domain"), DOMAIN_EXAMPLE);
        for tool in tools().iter().filter(|t| t.input_schema.get("properties").is_some_and(|p| p.get("domain").is_some())) {
            let example = &tool.input_schema["properties"]["domain"]["examples"][0];
            assert_eq!(example, DOMAIN_EXAMPLE, "{} must publish the shared domain example", tool.name);
        }
    }
}
