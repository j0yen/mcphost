//! PRD-mcphost-oauth-conformance-harness: an OAuth client simulator
//! faithful to the clients that matter (claude.ai, Claude Code, ChatGPT,
//! Cursor, VS Code) -- discovers through a 401, reads both metadata
//! documents, registers by CIMD or DCR, runs PKCE, exchanges, refreshes,
//! steps up on 403, checks the RFC 9207 `iss` parameter, and tries the
//! attack probes the 119-server study named. Every step returns a
//! [`StepRecord`]: `secrets never enter records` (requirement 1) -- no
//! function in this module ever places a code/token/secret VALUE into a
//! [`StepRecord`]'s `headers_of_interest` or `reason` fields, only status
//! codes, header presence/shape and fixed, human-written descriptions.
//!
//! This module compiles into the release binary (`mcphost oauth-probe`,
//! `src/cli/oauth_probe.rs`) as well as the gate's `tests/oauthconf_*.rs`
//! suite (via the thin re-export at `tests/support/oauthclient.rs`) --
//! goal 1: "a client simulator ... reusable by every OAuth PRD's tests."
//! [`run_all`] is the one entry point both callers share so the verdict
//! table can never drift between the gate and a live probe.
//!
//! AC1 lands the `discover_401`/`read_prm`/`read_as_metadata` steps plus
//! the `prm`/`iss`/generic-probe scenario runners; AC2 adds `register`;
//! AC3 adds the `--client` profiles and `authorize`'s pure request
//! builder; AC4 adds the network half of `authorize`, `consent`, `token`,
//! `iss_check`, `call`, `refresh`, `step_up`, and the attack probes.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Non-functional requirement: "the probe honours a 10 s per-step timeout".
const STEP_TIMEOUT: Duration = Duration::from_secs(10);
/// The tool every anonymous-401 / bearer-identity step calls: it always
/// requires a tenant (same probe `PRD-mcphost-oauth-resource-server`'s own
/// AC4 test uses), so a 401/invalid_token response is never confused with
/// "this particular tool doesn't need auth".
const PROBE_TOOL: &str = "host.state.get";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Unsupported,
    Fail,
}

/// requirement 1's structured step record. `headers_of_interest` and
/// `reason` are built only from response *shape* (status, header
/// presence, fixed strings) -- never from a body value that could be a
/// code or token (AC6).
#[derive(Debug, Clone, Serialize)]
pub struct StepRecord {
    pub step: String,
    pub status: Option<u16>,
    pub headers_of_interest: BTreeMap<String, String>,
    pub verdict: Verdict,
    pub reason: Option<String>,
}

impl StepRecord {
    fn pass(step: &str) -> Self {
        Self { step: step.to_string(), status: None, headers_of_interest: BTreeMap::new(), verdict: Verdict::Pass, reason: None }
    }

    fn unsupported(step: &str, reason: impl Into<String>) -> Self {
        Self {
            step: step.to_string(),
            status: None,
            headers_of_interest: BTreeMap::new(),
            verdict: Verdict::Unsupported,
            reason: Some(reason.into()),
        }
    }

    fn fail(step: &str, reason: impl Into<String>) -> Self {
        Self {
            step: step.to_string(),
            status: None,
            headers_of_interest: BTreeMap::new(),
            verdict: Verdict::Fail,
            reason: Some(reason.into()),
        }
    }

    fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }
}

fn random_url_safe(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&buf)
}

// ---- discover_401 ----------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Discover401Result {
    pub resource_metadata_url: Option<String>,
    pub scope: Option<String>,
}

async fn probe_call(http: &reqwest::Client, mcp_url: &str, bearer: Option<&str>) -> Result<reqwest::Response, reqwest::Error> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": PROBE_TOOL, "arguments": {"key": "oauthconf-probe"}},
    });
    // No `MCP-Protocol-Version` header: `tests/common/mod.rs`'s own
    // `post_with_mcp_name_override` (the helper PRD-mcphost-oauth-
    // resource-server's own 401-challenge tests use) omits it too --
    // rmcp's SEP-2243 `Mcp-Name` enforcement still applies via the header
    // below, but declaring a protocol version here trips a stricter
    // validation path this probe doesn't need.
    let mut req = http
        .post(mcp_url)
        .timeout(STEP_TIMEOUT)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", PROBE_TOOL)
        .json(&body);
    if let Some(token) = bearer {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    req.send().await
}

/// requirement 1's `discover_401`: an anonymous call to a tenant-requiring
/// tool, parsing the RFC 6750 `WWW-Authenticate: Bearer resource_metadata=
/// "...", scope="..."` challenge.
pub async fn discover_401(http: &reqwest::Client, mcp_url: &str) -> (StepRecord, Option<Discover401Result>) {
    let resp = match probe_call(http, mcp_url, None).await {
        Ok(r) => r,
        Err(e) => return (StepRecord::fail("discover_401", format!("request failed: {e}")), None),
    };
    let status = resp.status().as_u16();
    if status != 401 {
        return (
            StepRecord::unsupported("discover_401", "expected a 401 challenge, host did not challenge").with_status(status),
            None,
        );
    }
    let header = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let params = parse_bearer_challenge(&header);
    let mut headers_of_interest = BTreeMap::new();
    if !header.is_empty() {
        headers_of_interest.insert("WWW-Authenticate".to_string(), header);
    }
    let resource_metadata_url = params.get("resource_metadata").cloned();
    if resource_metadata_url.is_none() {
        return (
            StepRecord {
                step: "discover_401".to_string(),
                status: Some(status),
                headers_of_interest,
                verdict: Verdict::Unsupported,
                reason: Some("401 challenge carried no resource_metadata param".to_string()),
            },
            None,
        );
    }
    (
        StepRecord { step: "discover_401".to_string(), status: Some(status), headers_of_interest, verdict: Verdict::Pass, reason: None },
        Some(Discover401Result { resource_metadata_url, scope: params.get("scope").cloned() }),
    )
}

/// RFC 6750 `Bearer <param>="<value>", <param>="<value>"` challenge parser.
fn parse_bearer_challenge(header: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let rest = header.strip_prefix("Bearer ").unwrap_or(header);
    for part in rest.split(',') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
        }
    }
    out
}

// ---- read_prm ---------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PrmDoc {
    pub resource: String,
    pub authorization_servers: Vec<String>,
    pub scopes_supported: Vec<String>,
}

fn str_array(body: &Value, field: &str) -> Vec<String> {
    body.get(field)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// requirement 1's `read_prm`: fetches the RFC 9728 protected-resource
/// metadata document at the URL [`discover_401`] named.
pub async fn read_prm(http: &reqwest::Client, metadata_url: &str) -> (StepRecord, Option<PrmDoc>) {
    let resp = match http.get(metadata_url).timeout(STEP_TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => return (StepRecord::fail("read_prm", format!("request failed: {e}")), None),
    };
    let status = resp.status().as_u16();
    if status == 404 {
        return (StepRecord::unsupported("read_prm", "protected-resource metadata absent (404)").with_status(status), None);
    }
    if !resp.status().is_success() {
        return (StepRecord::fail("read_prm", "metadata endpoint refused").with_status(status), None);
    }
    let Ok(body) = resp.json::<Value>().await else {
        return (StepRecord::fail("read_prm", "metadata response was not JSON").with_status(status), None);
    };
    let resource = body.get("resource").and_then(Value::as_str).unwrap_or_default().to_string();
    if resource.is_empty() {
        return (StepRecord::fail("read_prm", "metadata document has no resource").with_status(status), None);
    }
    (
        StepRecord::pass("read_prm").with_status(status),
        Some(PrmDoc {
            resource,
            authorization_servers: str_array(&body, "authorization_servers"),
            scopes_supported: str_array(&body, "scopes_supported"),
        }),
    )
}

// ---- read_as_metadata -------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AsMetadata {
    pub issuer: String,
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: Option<String>,
    pub registration_endpoint: Option<String>,
    pub client_id_metadata_document_supported: bool,
    pub token_endpoint_auth_methods_supported: Vec<String>,
}

fn opt_str(body: &Value, field: &str) -> Option<String> {
    body.get(field).and_then(Value::as_str).map(str::to_string)
}

/// requirement 1's `read_as_metadata`: RFC 8414's own well-known path, then
/// the OIDC discovery path, tried against the authorization server's
/// origin in that order. `unsupported` (never `fail`) when neither exists
/// -- requirement 2's "an endpoint that answers 404 reads unsupported".
pub async fn read_as_metadata(http: &reqwest::Client, as_origin: &str) -> (StepRecord, Option<AsMetadata>) {
    for path in ["/.well-known/oauth-authorization-server", "/.well-known/openid-configuration"] {
        let url = format!("{}{path}", as_origin.trim_end_matches('/'));
        let Ok(resp) = http.get(&url).timeout(STEP_TIMEOUT).send().await else { continue };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(body) = resp.json::<Value>().await else { continue };
        let Some(issuer) = opt_str(&body, "issuer") else { continue };
        return (
            StepRecord::pass("read_as_metadata"),
            Some(AsMetadata {
                issuer,
                authorization_endpoint: opt_str(&body, "authorization_endpoint"),
                token_endpoint: opt_str(&body, "token_endpoint"),
                registration_endpoint: opt_str(&body, "registration_endpoint"),
                client_id_metadata_document_supported: body
                    .get("client_id_metadata_document_supported")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                token_endpoint_auth_methods_supported: str_array(&body, "token_endpoint_auth_methods_supported"),
            }),
        );
    }
    (StepRecord::unsupported("read_as_metadata", "no RFC 8414 or OIDC authorization-server metadata document found"), None)
}

// ---- register (AC2) ---------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegMethod {
    Cimd,
    Dcr,
}

#[derive(Debug, Clone)]
pub struct Registration {
    pub client_id: String,
    pub method: RegMethod,
}

/// requirement 1's `register`: CIMD (no network call -- the `client_id` IS
/// the metadata document's own URL) when the AS advertises both
/// `client_id_metadata_document_supported` and `"none"` among
/// `token_endpoint_auth_methods_supported`; otherwise DCR against the AS's
/// `registration_endpoint`; neither present is `unsupported` with a reason
/// (AC2).
pub async fn register(
    http: &reqwest::Client,
    as_meta: &AsMetadata,
    cimd_document_url: &str,
    application_type: &str,
) -> (StepRecord, Option<Registration>) {
    if as_meta.client_id_metadata_document_supported
        && as_meta.token_endpoint_auth_methods_supported.iter().any(|m| m == "none")
    {
        return (StepRecord::pass("register"), Some(Registration { client_id: cimd_document_url.to_string(), method: RegMethod::Cimd }));
    }
    let Some(reg_endpoint) = &as_meta.registration_endpoint else {
        return (
            StepRecord::unsupported(
                "register",
                "AS advertises neither client_id_metadata_document_supported+\"none\" nor a registration_endpoint",
            ),
            None,
        );
    };
    let resp = match http
        .post(reg_endpoint)
        .timeout(STEP_TIMEOUT)
        .json(&json!({"application_type": application_type, "redirect_uris": Vec::<String>::new()}))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return (StepRecord::fail("register", format!("DCR request failed: {e}")), None),
    };
    let status = resp.status().as_u16();
    if status == 404 {
        return (StepRecord::unsupported("register", "registration endpoint absent (404)").with_status(status), None);
    }
    if !resp.status().is_success() {
        return (StepRecord::fail("register", "DCR registration refused").with_status(status), None);
    }
    let Ok(body) = resp.json::<Value>().await else {
        return (StepRecord::fail("register", "DCR response was not JSON").with_status(status), None);
    };
    let Some(client_id) = opt_str(&body, "client_id") else {
        return (StepRecord::fail("register", "DCR response had no client_id").with_status(status), None);
    };
    (StepRecord::pass("register").with_status(status), Some(Registration { client_id, method: RegMethod::Dcr }))
}

// ---- client profiles (requirement 7) --------------------------------------

/// `--client <name>` -- requirement 7's "Claude-shaped defaults" table.
/// Only `Claude`'s redirect/scope behaviour is grounded in this PRD's own
/// Grounding note (claude.com/docs/connectors/building/authentication);
/// the other four are this harness's own placeholder native/hosted
/// defaults, not a claim about any real client's exact URI, pending a
/// research-table update the way `Claude`'s own entry is sourced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    Claude,
    ClaudeCode,
    ChatGpt,
    Cursor,
    VsCode,
}

impl ClientKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "claude-code" => Some(Self::ClaudeCode),
            "chatgpt" => Some(Self::ChatGpt),
            "cursor" => Some(Self::Cursor),
            "vscode" => Some(Self::VsCode),
            _ => None,
        }
    }

    pub fn redirect_uri(&self) -> &'static str {
        match self {
            Self::Claude => "https://claude.ai/api/mcp/auth_callback",
            Self::ClaudeCode => "http://localhost:0/callback",
            Self::ChatGpt => "https://chatgpt.com/aip/connector_platform_oauth_redirect",
            Self::Cursor => "cursor://anysphere.cursor-retrieval/oauth/callback",
            Self::VsCode => "vscode://ms-vscode.mcp/authredirect",
        }
    }

    pub fn application_type(&self) -> &'static str {
        match self {
            Self::Claude | Self::ChatGpt => "web",
            Self::ClaudeCode | Self::Cursor | Self::VsCode => "native",
        }
    }

    /// Grounding: claude.ai "appends offline_access" for a proactively
    /// refreshable connection. Every other client defaults the same until
    /// a research-table entry shows otherwise.
    pub fn wants_offline_access(&self) -> bool {
        true
    }
}

// ---- authorize (pure request-builder half, AC3) -----------------------------

/// RFC 7636 S256: `BASE64URL(SHA256(code_verifier))`.
pub fn pkce_s256_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[derive(Debug, Clone)]
pub struct AuthorizeRequest {
    pub url: String,
    pub params: BTreeMap<String, String>,
    pub code_verifier: String,
    pub state: String,
}

/// requirement 1's `authorize`, split into a pure builder (this function --
/// AC3 asserts on it directly, no network needed) and a network half a
/// later PRD adds once this host has an authorization endpoint to submit
/// to. Requested scopes come from the 401 challenge's own `scope` param
/// when present, else the PRM document's `scopes_supported`; `offline_
/// access` is appended once when the client profile wants it. PKCE is
/// always S256; `state` is always present.
#[allow(clippy::too_many_arguments)]
pub fn build_authorize_request(
    authorization_endpoint: &str,
    client_id: &str,
    client: ClientKind,
    resource: &str,
    challenge_scope: Option<&str>,
    prm_scopes_supported: &[String],
) -> AuthorizeRequest {
    let mut scopes: Vec<String> = match challenge_scope {
        Some(s) if !s.trim().is_empty() => s.split_whitespace().map(str::to_string).collect(),
        _ => prm_scopes_supported.to_vec(),
    };
    if client.wants_offline_access() && !scopes.iter().any(|s| s == "offline_access") {
        scopes.push("offline_access".to_string());
    }
    let code_verifier = random_url_safe(64);
    let state = random_url_safe(24);

    let mut params = BTreeMap::new();
    params.insert("response_type".to_string(), "code".to_string());
    params.insert("client_id".to_string(), client_id.to_string());
    params.insert("redirect_uri".to_string(), client.redirect_uri().to_string());
    params.insert("scope".to_string(), scopes.join(" "));
    params.insert("resource".to_string(), resource.to_string());
    params.insert("code_challenge".to_string(), pkce_s256_challenge(&code_verifier));
    params.insert("code_challenge_method".to_string(), "S256".to_string());
    params.insert("state".to_string(), state.clone());

    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    AuthorizeRequest { url: format!("{authorization_endpoint}?{query}"), params, code_verifier, state }
}

/// Minimal percent-encoding sufficient for this module's own param values
/// (scopes separated by a literal space; every other value is already
/// URL-safe base64/host/scheme text).
fn urlencode(s: &str) -> String {
    s.replace(' ', "%20")
}

/// requirement 1's `authorize` network half (AC4): GETs the built request.
/// This harness's own fake authorization server (test support only, never
/// the host -- `tests/support/fake_as.rs`) answers synchronously with
/// `{claim_code, consent_endpoint}` rather than an HTML consent page
/// (Non-goals: no browser automation); [`consent`] completes the
/// handshake.
pub async fn submit_authorize(http: &reqwest::Client, req: &AuthorizeRequest) -> (StepRecord, Option<PendingConsent>) {
    let resp = match http.get(&req.url).timeout(STEP_TIMEOUT).send().await {
        Ok(r) => r,
        Err(e) => return (StepRecord::fail("authorize", format!("request failed: {e}")), None),
    };
    let status = resp.status().as_u16();
    if status == 404 {
        return (StepRecord::unsupported("authorize", "authorization endpoint absent (404)").with_status(status), None);
    }
    if !resp.status().is_success() {
        return (StepRecord::fail("authorize", "authorize request refused").with_status(status), None);
    }
    let Ok(body) = resp.json::<Value>().await else {
        return (StepRecord::fail("authorize", "authorize response was not JSON").with_status(status), None);
    };
    let (Some(claim_code), Some(consent_endpoint)) = (opt_str(&body, "claim_code"), opt_str(&body, "consent_endpoint")) else {
        return (StepRecord::fail("authorize", "authorize response missing claim_code/consent_endpoint").with_status(status), None);
    };
    (StepRecord::pass("authorize").with_status(status), Some(PendingConsent { claim_code, consent_endpoint }))
}

#[derive(Debug, Clone)]
pub struct PendingConsent {
    pub claim_code: String,
    pub consent_endpoint: String,
}

#[derive(Debug, Clone)]
pub struct AuthorizeResponse {
    pub code: String,
    pub state: String,
    pub iss: Option<String>,
}

/// requirement 1's `consent`: posts the claim code [`submit_authorize`]
/// received to the consent form, receiving the final authorization
/// response (`code`, `state`, and RFC 9207's `iss` when the AS sends one).
pub async fn consent(http: &reqwest::Client, pending: &PendingConsent) -> (StepRecord, Option<AuthorizeResponse>) {
    let resp = match http
        .post(&pending.consent_endpoint)
        .timeout(STEP_TIMEOUT)
        .json(&json!({"claim_code": pending.claim_code}))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return (StepRecord::fail("consent", format!("request failed: {e}")), None),
    };
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return (StepRecord::fail("consent", "consent form refused the claim code").with_status(status), None);
    }
    let Ok(body) = resp.json::<Value>().await else {
        return (StepRecord::fail("consent", "consent response was not JSON").with_status(status), None);
    };
    let (Some(code), Some(state)) = (opt_str(&body, "code"), opt_str(&body, "state")) else {
        return (StepRecord::fail("consent", "consent response missing code/state").with_status(status), None);
    };
    (StepRecord::pass("consent").with_status(status), Some(AuthorizeResponse { code, state, iss: opt_str(&body, "iss") }))
}

/// requirement 1's `iss_check` (RFC 9207): the authorization response's
/// own `iss` must equal the authorization server's own metadata `issuer`
/// -- the mix-up-attack defense a client applies to every redirect before
/// ever exchanging the code.
pub fn iss_check(authorize_response: &AuthorizeResponse, expected_issuer: &str) -> StepRecord {
    match &authorize_response.iss {
        Some(iss) if iss == expected_issuer => StepRecord::pass("iss_check"),
        Some(_) => StepRecord::fail("iss_check", "authorization response iss did not match the AS's own issuer"),
        None => StepRecord::unsupported("iss_check", "authorization response carried no iss parameter (RFC 9207)"),
    }
}

// ---- token / call / refresh / step_up (AC4) ---------------------------------

#[derive(Debug, Clone)]
pub struct TokenResult {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

pub struct TokenRequestArgs<'a> {
    pub token_endpoint: &'a str,
    pub code: &'a str,
    pub code_verifier: Option<&'a str>,
    pub redirect_uri: &'a str,
    pub client_id: &'a str,
    pub state: Option<&'a str>,
}

fn token_form<'a>(args: &TokenRequestArgs<'a>) -> Vec<(&'a str, &'a str)> {
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", args.code),
        ("redirect_uri", args.redirect_uri),
        ("client_id", args.client_id),
    ];
    if let Some(v) = args.code_verifier {
        form.push(("code_verifier", v));
    }
    if let Some(s) = args.state {
        form.push(("state", s));
    }
    form
}

/// requirement 1's `token`: the authorization_code grant. Never places
/// `access_token`/`refresh_token` into the returned [`StepRecord`] (AC6) --
/// they live only in the returned [`TokenResult`], a runtime value the
/// caller must not serialize into a record or receipt.
pub async fn token(http: &reqwest::Client, args: TokenRequestArgs<'_>) -> (StepRecord, Option<TokenResult>) {
    let form = token_form(&args);
    let resp = match http.post(args.token_endpoint).timeout(STEP_TIMEOUT).form(&form).send().await {
        Ok(r) => r,
        Err(e) => return (StepRecord::fail("token", format!("request failed: {e}")), None),
    };
    let status = resp.status().as_u16();
    if status == 404 {
        return (StepRecord::unsupported("token", "token endpoint absent (404)").with_status(status), None);
    }
    let Ok(body) = resp.json::<Value>().await else {
        return (StepRecord::fail("token", "token response was not JSON").with_status(status), None);
    };
    let access_token = opt_str(&body, "access_token");
    let refresh_token = opt_str(&body, "refresh_token");
    let record = if status == 200 && access_token.is_some() {
        StepRecord::pass("token").with_status(status)
    } else {
        StepRecord::fail("token", "token endpoint did not issue an access token").with_status(status)
    };
    (record, Some(TokenResult { access_token, refresh_token }))
}

/// requirement 5's attack probes: each redeems a previously-issued code
/// with exactly one dimension tampered, `pass` only when the authorization
/// server's response is an error carrying no token (AC4). Never places
/// the code/verifier/state VALUES into the record -- only which probe ran
/// and whether the AS refused.
async fn attack_expect_refusal(http: &reqwest::Client, step: &str, args: TokenRequestArgs<'_>) -> StepRecord {
    let form = token_form(&args);
    let resp = match http.post(args.token_endpoint).timeout(STEP_TIMEOUT).form(&form).send().await {
        Ok(r) => r,
        Err(e) => return StepRecord::fail(step, format!("request failed: {e}")),
    };
    let status = resp.status().as_u16();
    let Ok(body) = resp.json::<Value>().await else {
        return StepRecord::fail(step, "attack response was not JSON").with_status(status);
    };
    let has_token = body.get("access_token").and_then(Value::as_str).is_some();
    let is_error = status >= 400 && body.get("error").and_then(Value::as_str).is_some();
    if is_error && !has_token {
        StepRecord::pass(step).with_status(status)
    } else {
        StepRecord::fail(step, "authorization server did not refuse this attack").with_status(status)
    }
}

/// AC4: replays a code+verifier already redeemed once.
pub async fn attack_replay(http: &reqwest::Client, args: TokenRequestArgs<'_>) -> StepRecord {
    attack_expect_refusal(http, "attack_replay", args).await
}

/// AC4: redeems a fresh code with no `code_verifier` at all -- presenting
/// a PKCE-issued code without ever proving possession of the verifier.
pub async fn attack_pkce_downgrade(http: &reqwest::Client, args: TokenRequestArgs<'_>) -> StepRecord {
    let mut args = args;
    args.code_verifier = None;
    attack_expect_refusal(http, "attack_pkce_downgrade", args).await
}

/// AC4: redeems a fresh code carrying a `state` different from the one
/// the authorization response actually returned.
pub async fn attack_state_tamper(http: &reqwest::Client, args: TokenRequestArgs<'_>, tampered_state: &str) -> StepRecord {
    let mut args = args;
    args.state = Some(tampered_state);
    attack_expect_refusal(http, "attack_state_tamper", args).await
}

/// AC4: redeems a fresh code as a `client_id` other than the one the
/// authorize request actually registered.
pub async fn attack_cross_client_redeem(http: &reqwest::Client, args: TokenRequestArgs<'_>, other_client_id: &str) -> StepRecord {
    let mut args = args;
    args.client_id = other_client_id;
    attack_expect_refusal(http, "attack_cross_client_redeem", args).await
}

/// requirement 1's `call`: an authenticated re-run of the same probe call
/// [`discover_401`] opened with, proving the access token actually admits
/// the request that was originally refused.
pub async fn call(http: &reqwest::Client, mcp_url: &str, access_token: &str) -> StepRecord {
    match probe_call(http, mcp_url, Some(access_token)).await {
        Ok(resp) if resp.status() == 200 => StepRecord::pass("call"),
        Ok(resp) => StepRecord::fail("call", "authenticated call was refused").with_status(resp.status().as_u16()),
        Err(e) => StepRecord::fail("call", format!("request failed: {e}")),
    }
}

/// requirement 1's `refresh`: the refresh_token grant.
pub async fn refresh(http: &reqwest::Client, token_endpoint: &str, refresh_token: &str, client_id: &str) -> (StepRecord, Option<TokenResult>) {
    let form = [("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id)];
    let resp = match http.post(token_endpoint).timeout(STEP_TIMEOUT).form(&form).send().await {
        Ok(r) => r,
        Err(e) => return (StepRecord::fail("refresh", format!("request failed: {e}")), None),
    };
    let status = resp.status().as_u16();
    if status == 404 {
        return (StepRecord::unsupported("refresh", "token endpoint absent (404)").with_status(status), None);
    }
    let Ok(body) = resp.json::<Value>().await else {
        return (StepRecord::fail("refresh", "refresh response was not JSON").with_status(status), None);
    };
    let access_token = opt_str(&body, "access_token");
    let record = if status == 200 && access_token.is_some() {
        StepRecord::pass("refresh").with_status(status)
    } else {
        StepRecord::fail("refresh", "refresh grant did not issue an access token").with_status(status)
    };
    (record, Some(TokenResult { access_token, refresh_token: opt_str(&body, "refresh_token") }))
}

/// requirement 1's `step_up`: on 403, a real full flow re-authorizes with
/// the union of scopes; this step only detects the 403 trigger itself.
pub async fn step_up(http: &reqwest::Client, mcp_url: &str, access_token: &str) -> StepRecord {
    match probe_call(http, mcp_url, Some(access_token)).await {
        Ok(resp) if resp.status() == 403 => StepRecord::pass("step_up"),
        Ok(resp) if resp.status() == 200 => StepRecord::unsupported("step_up", "call succeeded without a 403 step-up challenge"),
        Ok(resp) => StepRecord::fail("step_up", "unexpected status").with_status(resp.status().as_u16()),
        Err(e) => StepRecord::fail("step_up", format!("request failed: {e}")),
    }
}

// ---- scenario gold + the shared runner (requirement 2/3/4) ------------------

/// requirement 2: gold in data, not in test code.
pub const SCENARIOS_TOML: &str = include_str!("../tests/oauthconf/scenarios.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub family: String,
    pub owner_prd: String,
    pub steps: Vec<String>,
    pub expected: String,
}

#[derive(Debug, Deserialize)]
struct ScenarioFile {
    scenario: Vec<Scenario>,
}

/// Panics on a malformed `scenarios.toml` -- a compile-time-embedded,
/// committed file, so a parse failure is a bug in this crate, not
/// something a caller can recover from at runtime.
pub fn load_scenarios() -> Vec<Scenario> {
    let file: ScenarioFile = toml::from_str(SCENARIOS_TOML).expect("tests/oauthconf/scenarios.toml must parse"); // allowlist: compile-time-embedded, committed file (include_str!); a parse failure here is a build-time bug in this crate, not external/runtime input
    file.scenario
}

#[derive(Debug, Clone, Serialize)]
pub struct ScenarioResult {
    pub name: String,
    pub family: String,
    pub verdict: Verdict,
    pub records: Vec<StepRecord>,
}

/// requirement 3/4: runs every scenario in [`load_scenarios`] against
/// `mcp_url`, producing the identical verdict table whether called from
/// the in-process gate test or the `oauth-probe` CLI (goal 1/3).
///
/// Every family other than `prm`/`iss` shares one generic runner: probe
/// through `read_as_metadata`, then stop -- unconditionally `unsupported`
/// past that point, even when `read_as_metadata` itself passes (as it now
/// does once `mcphost-hosted-authorization-server` is on this tree), since
/// this generic arm has no family-specific runner for `register`/
/// `authorize`/`token`/attack probes yet. A future feature PRD that
/// lands that support (and, per goal 3, updates
/// this scenario's `owner_prd` in `tests/oauthconf/scenarios.toml`) will
/// need a family-specific runner arm here alongside `prm`/`iss` -- this
/// generic arm alone cannot prove PKCE/DCR/CIMD/attack-probe correctness,
/// only that the capability doesn't exist yet.
pub async fn run_all(http: &reqwest::Client, mcp_url: &str) -> Vec<ScenarioResult> {
    let mut out = Vec::new();
    for scenario in load_scenarios() {
        let (verdict, records) = match scenario.family.as_str() {
            "prm" => run_prm_probe(http, mcp_url).await,
            "iss" => {
                let record = run_iss_probe(http, mcp_url).await;
                let verdict = record.verdict;
                (verdict, vec![record])
            }
            _ => run_generic_probe(http, mcp_url).await,
        };
        out.push(ScenarioResult { name: scenario.name, family: scenario.family, verdict, records });
    }
    out
}

async fn run_prm_probe(http: &reqwest::Client, mcp_url: &str) -> (Verdict, Vec<StepRecord>) {
    let (rec, discover) = discover_401(http, mcp_url).await;
    let verdict = rec.verdict;
    let mut records = vec![rec];
    let Some(discover) = discover else {
        return (verdict, records);
    };
    let Some(metadata_url) = discover.resource_metadata_url else {
        records.push(StepRecord::unsupported("read_prm", "no resource_metadata url from the 401 challenge"));
        return (Verdict::Unsupported, records);
    };
    let (rec, _) = read_prm(http, &metadata_url).await;
    let verdict = rec.verdict;
    records.push(rec);
    (verdict, records)
}

/// The `iss` family's own scenario: an unregistered issuer must be
/// rejected (RFC 9207's mix-up-attack defense), the resource-server-side
/// half of `iss_check` -- no authorize/token endpoint is needed for this
/// half, so it is provable on this tree today.
async fn run_iss_probe(http: &reqwest::Client, mcp_url: &str) -> StepRecord {
    let token = mint_unregistered_issuer_probe_token();
    let resp = match probe_call(http, mcp_url, Some(&token)).await {
        Ok(r) => r,
        Err(e) => return StepRecord::fail("iss_check", format!("request failed: {e}")),
    };
    let status = resp.status().as_u16();
    let Ok(body) = resp.json::<Value>().await else {
        return StepRecord::fail("iss_check", "response was not JSON").with_status(status);
    };
    let error_code = body["error"]["data"]["error_code"].as_str().unwrap_or_default();
    let error_description = body["error"]["data"]["error_description"].as_str().unwrap_or_default();
    if status == 401 && error_code == "invalid_token" && error_description == "unknown_issuer" {
        StepRecord::pass("iss_check").with_status(status)
    } else {
        StepRecord::fail("iss_check", "an unregistered issuer's bearer token was not rejected as unknown_issuer").with_status(status)
    }
}

/// A syntactically valid but never-registered-anywhere bearer JWT: HS256
/// with a random per-call secret is enough, since `validate_bearer` looks
/// up the (unregistered, guaranteed-unique) `iss` claim and rejects before
/// ever resolving a decoding key or checking the algorithm.
fn mint_unregistered_issuer_probe_token() -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    let mut secret = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    let iss = format!("https://oauthconf-probe.invalid/{}", random_url_safe(8));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let claims = json!({"iss": iss, "aud": "mcphost", "sub": "oauthconf-probe", "iat": now, "exp": now + 300});
    let header = Header::new(Algorithm::HS256);
    encode(&header, &claims, &EncodingKey::from_secret(&secret)).expect("sign probe token")
}

async fn run_generic_probe(http: &reqwest::Client, mcp_url: &str) -> (Verdict, Vec<StepRecord>) {
    let (rec, discover) = discover_401(http, mcp_url).await;
    let verdict = rec.verdict;
    let mut records = vec![rec];
    let Some(discover) = discover else {
        return (verdict, records);
    };
    let Some(metadata_url) = discover.resource_metadata_url else {
        records.push(StepRecord::unsupported("read_prm", "no resource_metadata url from the 401 challenge"));
        return (Verdict::Unsupported, records);
    };
    let (rec, prm) = read_prm(http, &metadata_url).await;
    let verdict = rec.verdict;
    records.push(rec);
    let Some(prm) = prm else {
        return (verdict, records);
    };
    let as_origin = prm.authorization_servers.first().cloned().unwrap_or(prm.resource);
    let (rec, meta) = read_as_metadata(http, &as_origin).await;
    let verdict = rec.verdict;
    records.push(rec);
    if meta.is_some() {
        // PRD-mcphost-hosted-authorization-server (rebased onto, 2026-09-26)
        // gave this host its own AS metadata document, so read_as_metadata
        // itself can now pass -- but this generic arm still has no
        // family-specific runner for register/authorize/token/attack probes
        // (see this fn's own doc comment), so it cannot prove anything past
        // this point. One passing step must not read as a pass for the
        // whole (as yet unimplemented) family.
        records.push(StepRecord::unsupported(
            "register",
            "generic probe has no family-specific runner past read_as_metadata yet",
        ));
        return (Verdict::Unsupported, records);
    }
    (verdict, records)
}
