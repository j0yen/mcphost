//! PRD-mcphost-hosted-authorization-server: mcphost becomes an OAuth 2.1
//! authorization server for its own tenants -- discovery metadata
//! (`/.well-known/oauth-authorization-server`, `/.well-known/openid-configuration`,
//! `/.well-known/jwks.json`), client identification (CIMD and DCR),
//! `/oauth/authorize`, `/oauth/token`, `/oauth/register`, `/oauth/revoke`,
//! and the `host.oauth.grants`/`host.oauth.grant_revoke` tenant tools.
//! `oauth.rs` keeps owning bring-your-own-issuer bearer validation and its
//! own `host.oauth.issuer_*`/`admin.oauth.issuers` tools; this module owns
//! only what's new here, and calls into `oauth.rs`'s `OauthCaller` shape
//! for the built-in-issuer bearer path so `handler.rs`'s `resolve_auth`
//! needs no second call site.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::claim::{html_escape, html_response, page};
use crate::errors::AppError;
use crate::state::AppState;

const KEY_FILE_NAME: &str = "authz_signing_key.der";

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// This host's own ES256 authorization-server signing key (requirement 5):
/// a PKCS8 DER private key file (0600) in the data dir, generated on first
/// start and reloaded on every restart thereafter, unless
/// `$MCPHOST_AUTHZ_KEY_ROTATE=1` is set for this process, in which case a
/// fresh key overwrites it. `kid` is derived from the public key itself
/// (the first 16 hex characters of its SHA-256 digest) rather than stored
/// separately, so a rotated key always publishes a distinct `kid` with no
/// extra bookkeeping.
#[derive(Clone)]
pub struct AuthzSigningKey {
    pub kid: String,
    encoding_key: EncodingKey,
    x: String,
    y: String,
}

impl AuthzSigningKey {
    fn from_pkcs8_der(der: &[u8]) -> Result<Self, AppError> {
        let rng = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der, &rng)
            .map_err(|e| AppError::Internal(format!("authz signing key: invalid pkcs8: {e}")))?;
        let public = pair.public_key().as_ref();
        if public.len() != 65 || public[0] != 0x04 {
            return Err(AppError::Internal(
                "authz signing key: unexpected public key point".to_string(),
            ));
        }
        let x = URL_SAFE_NO_PAD.encode(&public[1..33]);
        let y = URL_SAFE_NO_PAD.encode(&public[33..65]);
        let mut hasher = Sha256::new();
        hasher.update(public);
        let kid = to_hex(&hasher.finalize())[..16].to_string();
        Ok(Self { kid, encoding_key: EncodingKey::from_ec_der(der), x, y })
    }

    /// requirement 5: generated on first start into the data dir, backed up
    /// by the existing off-box backup (a plain file under `data_dir`, same
    /// as `mcphost.db` itself); rotatable by `$MCPHOST_AUTHZ_KEY_ROTATE=1`.
    pub fn load_or_generate(data_dir: &Path) -> Result<Self, AppError> {
        std::fs::create_dir_all(data_dir)
            .map_err(|e| AppError::Storage(format!("cannot create data dir: {e}")))?;
        let path = data_dir.join(KEY_FILE_NAME);
        let rotate = std::env::var("MCPHOST_AUTHZ_KEY_ROTATE").as_deref() == Ok("1");
        if !rotate && path.exists() {
            let der = std::fs::read(&path)
                .map_err(|e| AppError::Storage(format!("cannot read authz signing key: {e}")))?;
            return Self::from_pkcs8_der(&der);
        }
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| AppError::Internal("authz signing key: keygen failed".to_string()))?;
        let der = doc.as_ref();
        std::fs::write(&path, der)
            .map_err(|e| AppError::Storage(format!("cannot write authz signing key: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| AppError::Storage(format!("cannot chmod authz signing key: {e}")))?;
        }
        Self::from_pkcs8_der(der)
    }

    fn jwk(&self) -> Value {
        json!({"kty": "EC", "kid": self.kid, "crv": "P-256", "use": "sig", "alg": "ES256", "x": self.x, "y": self.y})
    }

    /// `GET /.well-known/jwks.json` (requirement 5).
    pub fn jwks_document(&self) -> Value {
        json!({"keys": [self.jwk()]})
    }

    pub fn decoding_key(&self) -> Result<DecodingKey, AppError> {
        DecodingKey::from_ec_components(&self.x, &self.y)
            .map_err(|e| AppError::Internal(format!("authz signing key: bad decoding key: {e}")))
    }

    /// Signs an access-token JWT's claims with this key, `ES256`, `kid` set.
    pub fn sign(&self, claims: &Value) -> Result<String, AppError> {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        jsonwebtoken::encode(&header, claims, &self.encoding_key)
            .map_err(|e| AppError::Internal(format!("authz: failed to sign access token: {e}")))
    }
}

/// requirement 3/Technical considerations: per-tenant resource URIs aren't
/// accepted until PRD-mcphost-tenant-resource-metadata lands -- until then
/// this is the one and only acceptable `resource`, a single constant.
pub fn root_resource(state: &AppState) -> String {
    format!("{}/mcp", state.public_url.trim_end_matches('/'))
}

/// requirement 1 (AC1): the RFC 8414 authorization-server metadata document
/// -- also served verbatim at `/.well-known/openid-configuration` (the OIDC
/// discovery document this deployment needs no separate fields for, since
/// Non-goals rules out id_tokens/userinfo).
pub fn authorization_server_metadata(state: &AppState) -> Value {
    let issuer = state.public_url.trim_end_matches('/').to_string();
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "revocation_endpoint": format!("{issuer}/oauth/revoke"),
        "jwks_uri": format!("{issuer}/.well-known/jwks.json"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
        "scopes_supported": ["mcp"],
        "client_id_metadata_document_supported": true,
    })
}

// ---- client identification (CIMD + DCR) -----------------------------------

/// requirement 2(a): a `client_id` is CIMD when it's an `https` URL --
/// this is the one thing that can never collide with a DCR-issued
/// `client_id` (see [`crate::db::generate_dcr_client_id`]), which never
/// looks like a URL at all. Test builds also accept a plain `http` URL
/// (same `test-support`-gated relaxation as `kinds::http::HttpKind`'s own
/// `allow_loopback`): a real deployment only ever fetches CIMD documents
/// over TLS, but the test suite has no CA-signed cert to hand a `wiremock`
/// server, so exercising the CIMD path at all needs this escape hatch.
fn is_cimd_client_id(client_id: &str) -> bool {
    client_id.starts_with("https://") || (cfg!(feature = "test-support") && client_id.starts_with("http://"))
}

/// requirement 2(a): the CIMD document's own shape -- only the fields this
/// crate reads; an unrecognized extra field is ignored, same tolerance
/// `oauth.rs`'s hand-rolled `JwkDoc` gives a JWKS document.
#[derive(Debug, Clone, Deserialize)]
struct CimdDoc {
    client_id: String,
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
}

const CIMD_MAX_BYTES: usize = 64 * 1024;
const CIMD_FETCH_TIMEOUT_SECS: u64 = 5;
const CIMD_CACHE_TTL_SECS: u64 = 3600;

struct CachedCimd {
    doc: Option<CimdDoc>,
    fetched_at: Instant,
}

/// requirement 2(a): the CIMD fetch-once-cache-1h cache, keyed by URL --
/// same shape as [`crate::oauth::JwksCache`]. A failed fetch (unreachable,
/// non-JSON, `client_id` mismatch) is cached too (as `None`), so a client
/// hammering `/oauth/authorize` with a broken CIMD URL doesn't turn into a
/// live outbound request on every single attempt.
#[derive(Clone, Default)]
pub struct CimdCache {
    inner: Arc<Mutex<HashMap<String, CachedCimd>>>,
}

impl CimdCache {
    pub fn new() -> Self {
        Self::default()
    }

    async fn resolve(&self, url: &str) -> Option<CimdDoc> {
        let cached = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.get(url).and_then(|c| {
                (Instant::now().duration_since(c.fetched_at) < Duration::from_secs(CIMD_CACHE_TTL_SECS))
                    .then(|| c.doc.clone())
            })
        };
        if let Some(doc) = cached {
            return doc;
        }
        let doc = fetch_cimd(url).await;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(url.to_string(), CachedCimd { doc: doc.clone(), fetched_at: Instant::now() });
        doc
    }
}

/// requirement 2(a): a dedicated client, never `AppState::http_client` --
/// CIMD fetch must never follow a redirect (an attacker-controlled
/// redirect target is exactly the open-redirect flaw class this PRD's
/// grounding names), which is a client-construction-time policy `reqwest`
/// has no per-request override for.
fn cimd_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("build CIMD http client")
    })
}

/// requirement 2(a): "no private-network targets" -- a literal-host check
/// plus one DNS resolution vetted against the same disallowed ranges
/// `kinds::http`'s own SSRF guard uses. Unlike that module's
/// `VettingResolver`, this is a pre-connect check, not the resolver
/// `reqwest` itself connects through, so a DNS answer that changes between
/// this check and the actual connect (a rebinding race) is not fully closed
/// -- an accepted tradeoff for a document fetch that only ever populates a
/// consent page, not a credential exchange.
async fn cimd_host_allowed(url: &str) -> bool {
    // Same `test-support`-gated relaxation as `is_cimd_client_id`: a test
    // harness's CIMD fixture is necessarily a loopback `wiremock` server, so
    // a real deployment's loopback/private-network refusal would make the
    // CIMD path untestable at all. Production (no `test-support` feature)
    // keeps loopback blocked unconditionally, same as `kinds::http::HttpKind`'s
    // own default (`allow_loopback: false`).
    let allow_loopback = cfg!(feature = "test-support");
    let Some((_, host)) = crate::kinds::http::split_scheme_host(url) else {
        return false;
    };
    if crate::kinds::http::is_disallowed_literal_host(host, "", allow_loopback) {
        return false;
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        // Already vetted as a literal IP above; no DNS lookup to do.
        return true;
    }
    let Ok(lookup) = crate::kinds::http::HickoryLookup::new() else {
        return false;
    };
    match crate::kinds::http::NameLookup::lookup(&lookup, host.to_string()).await {
        Ok(ips) if ips.is_empty() => false,
        Ok(ips) => !ips
            .into_iter()
            .any(|ip| crate::kinds::http::is_disallowed_ip(ip, allow_loopback)),
        Err(_) => false,
    }
}

/// requirement 2(a): fetches and validates a CIMD document -- `None` on
/// any failure (unreachable, too large, non-JSON, or `client_id` not equal
/// to the URL it was fetched from), which every caller treats as "this
/// client cannot be identified" (AC2/AC6's `invalid_request`).
async fn fetch_cimd(url: &str) -> Option<CimdDoc> {
    if !cimd_host_allowed(url).await {
        return None;
    }
    let resp = cimd_http_client()
        .get(url)
        .timeout(Duration::from_secs(CIMD_FETCH_TIMEOUT_SECS))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    if resp.content_length().is_some_and(|n| n > CIMD_MAX_BYTES as u64) {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > CIMD_MAX_BYTES {
        return None;
    }
    let doc: CimdDoc = serde_json::from_slice(&bytes).ok()?;
    if doc.client_id != url || doc.redirect_uris.is_empty() {
        return None;
    }
    Some(doc)
}

/// A client identified either by CIMD document or DCR registration --
/// [`identify_client`]'s success shape, and what `/oauth/authorize`'s
/// redirect-uri/consent-naming logic reads from either source uniformly.
#[derive(Debug, Clone)]
pub struct ClientIdentity {
    pub client_id: String,
    pub client_name: Option<String>,
    /// `"cimd"` or `"dcr"` -- also `oauth_grants.method`/`host.oauth.grants`'s
    /// own `method` field (requirement 7).
    pub method: &'static str,
    pub redirect_uris: Vec<String>,
    pub application_type: String,
    /// `Some` only for a confidential DCR client (requirement 2(b)).
    pub client_secret_hash: Option<String>,
}

/// requirement 2: resolves a `client_id` to its identity, whichever of
/// CIMD or DCR it is -- `None` when it can't be identified at all (unknown
/// DCR `client_id`, or a CIMD URL that doesn't fetch/validate).
pub async fn identify_client(state: &AppState, client_id: &str) -> Option<ClientIdentity> {
    if is_cimd_client_id(client_id) {
        let doc = state.cimd_cache.resolve(client_id).await?;
        return Some(ClientIdentity {
            client_id: client_id.to_string(),
            client_name: doc.client_name,
            method: "cimd",
            redirect_uris: doc.redirect_uris,
            application_type: "web".to_string(),
            client_secret_hash: None,
        });
    }
    let row = state.db.find_oauth_client_by_id(client_id.to_string()).await.ok().flatten()?;
    Some(ClientIdentity {
        client_id: row.client_id,
        client_name: row.client_name,
        method: "dcr",
        redirect_uris: row.redirect_uris,
        application_type: row.application_type,
        client_secret_hash: row.client_secret_hash,
    })
}

/// requirement 2(b): for a native DCR client, a registered redirect URI
/// with no port (`http://127.0.0.1/cb`) matches a request naming ANY port
/// on the same loopback host (`http://127.0.0.1:53211/cb`, AC3) --
/// implemented by stripping the port from both sides and comparing, rather
/// than a wildcard match, so a scheme/path/query mismatch still refuses.
fn strip_loopback_port(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let path_start = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..path_start];
    let tail = &rest[path_start..];
    let host = authority.split(':').next().unwrap_or(authority);
    if host == "127.0.0.1" || host == "localhost" {
        Some(format!("{scheme}://{host}{tail}"))
    } else {
        None
    }
}

/// requirement 3: `redirect_uri` must be exactly one of the identified
/// client's registered URIs -- except a native DCR client's loopback
/// redirect, which additionally matches on any port (requirement 2(b)).
fn redirect_uri_allowed(identity: &ClientIdentity, requested: &str) -> bool {
    if identity.redirect_uris.iter().any(|r| r == requested) {
        return true;
    }
    if identity.method != "dcr" || identity.application_type != "native" {
        return false;
    }
    let Some(requested_stripped) = strip_loopback_port(requested) else {
        return false;
    };
    identity.redirect_uris.iter().any(|r| {
        r == &requested_stripped || strip_loopback_port(r).as_deref() == Some(requested_stripped.as_str())
    })
}

// ---- consent page ----------------------------------------------------------

/// `GET`/`POST /oauth/authorize`'s query/form fields, shared by both --
/// the consent page's `POST` carries every one of these back as hidden
/// fields (requirement 6: "consent cannot be skipped by any parameter",
/// so every one of them is re-validated on the `POST`, never trusted
/// as-is from the hidden field alone).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthorizeParams {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub scope: Option<String>,
    pub resource: Option<String>,
}

fn opt_str(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("")
}

fn render_consent_page(client_name: &str, resource: &str, params: &AuthorizeParams, error: Option<&str>) -> String {
    let error_html = error
        .map(|e| format!("<p class=\"err\">{}</p>", html_escape(e)))
        .unwrap_or_default();
    page(
        "mcphost — authorize",
        &format!(
            "<h1>Authorize {client}</h1>\
             <p>{client} is requesting access to <code>{resource}</code> on your behalf.</p>\
             {error_html}\
             <form method=\"post\" action=\"/oauth/authorize\">\
             <input type=\"hidden\" name=\"response_type\" value=\"{response_type}\">\
             <input type=\"hidden\" name=\"client_id\" value=\"{client_id}\">\
             <input type=\"hidden\" name=\"redirect_uri\" value=\"{redirect_uri}\">\
             <input type=\"hidden\" name=\"code_challenge\" value=\"{code_challenge}\">\
             <input type=\"hidden\" name=\"code_challenge_method\" value=\"{code_challenge_method}\">\
             <input type=\"hidden\" name=\"state\" value=\"{state}\">\
             <input type=\"hidden\" name=\"scope\" value=\"{scope}\">\
             <input type=\"hidden\" name=\"resource\" value=\"{resource_attr}\">\
             <p>Prove you own this tenant, with its key:</p>\
             <input type=\"password\" name=\"tenant_key\" placeholder=\"tenant key\">\
             <p>...or with a claim code from your email:</p>\
             <input type=\"text\" name=\"claim_code\" placeholder=\"claim code\">\
             <button type=\"submit\">Approve</button>\
             </form>",
            client = html_escape(client_name),
            resource = html_escape(resource),
            response_type = html_escape(opt_str(&params.response_type)),
            client_id = html_escape(opt_str(&params.client_id)),
            redirect_uri = html_escape(opt_str(&params.redirect_uri)),
            code_challenge = html_escape(opt_str(&params.code_challenge)),
            code_challenge_method = html_escape(opt_str(&params.code_challenge_method)),
            state = html_escape(opt_str(&params.state)),
            scope = html_escape(opt_str(&params.scope)),
            resource_attr = html_escape(opt_str(&params.resource)),
        ),
    )
}

/// requirement 3/AC2/AC6: every failure that has no trusted `redirect_uri`
/// to send the caller back to yet (client identification itself failed, or
/// `redirect_uri` isn't a registered one) renders inline -- never a
/// redirect, since redirecting to an unvalidated URI is exactly the
/// open-redirect flaw class this PRD's grounding names.
fn render_inline_error(code: &str, message: &str) -> Response {
    html_response(
        StatusCode::BAD_REQUEST,
        page(
            "mcphost — authorize",
            &format!("<h1>{}</h1><p>{}</p>", html_escape(code), html_escape(message)),
        ),
    )
}

/// requirement 3/AC6: once `redirect_uri` is a registered one, every later
/// validation failure (bad PKCE, bad `resource`) redirects there with
/// `error`/`state` query parameters instead of rendering inline --
/// `reqwest::Url`'s `query_pairs_mut` percent-encodes `state` correctly and
/// appends to any query string `redirect_uri` already carries.
fn error_redirect(redirect_uri: &str, error: &str, state_param: Option<&str>) -> Response {
    let Ok(mut url) = reqwest::Url::parse(redirect_uri) else {
        return render_inline_error("invalid_request", "redirect_uri is malformed");
    };
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("error", error);
        if let Some(s) = state_param {
            pairs.append_pair("state", s);
        }
    }
    Redirect::to(url.as_str()).into_response()
}

/// requirement 3/AC2/AC6: every validation `GET`/`POST /oauth/authorize`
/// share -- client identification, redirect-uri registration, then (once
/// `redirect_uri` is trusted) PKCE/state/scope/resource. `Ok` carries the
/// identified client, the now-trusted `redirect_uri`, and the validated
/// `resource`; `Err` is an already-built response (inline for the first
/// two failure classes, a redirect to the now-trusted `redirect_uri` for
/// the rest) -- requirement 6: "consent cannot be skipped by any
/// parameter", so `POST /oauth/authorize` runs this exact same function
/// over its own (re-submitted, not blindly trusted) fields rather than
/// trusting whatever the hidden form fields said.
async fn validate_authorize_params(
    state: &AppState,
    params: &AuthorizeParams,
) -> Result<(ClientIdentity, String, String), Box<Response>> {
    let Some(client_id) = params.client_id.clone().filter(|s| !s.is_empty()) else {
        return Err(Box::new(render_inline_error("invalid_request", "client_id is required")));
    };
    let Some(identity) = identify_client(state, &client_id).await else {
        return Err(Box::new(render_inline_error(
            "invalid_request",
            "client_id could not be identified (unknown, or its CIMD document did not validate)",
        )));
    };
    let Some(redirect_uri) = params.redirect_uri.clone().filter(|s| !s.is_empty()) else {
        return Err(Box::new(render_inline_error("invalid_request", "redirect_uri is required")));
    };
    if !redirect_uri_allowed(&identity, &redirect_uri) {
        return Err(Box::new(render_inline_error(
            "invalid_request",
            "redirect_uri is not registered for this client",
        )));
    }

    // `redirect_uri` is now a trusted, registered value -- every failure
    // from here on redirects there instead of rendering inline.
    if params.response_type.as_deref() != Some("code") {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_request", params.state.as_deref())));
    }
    if params.code_challenge.as_deref().unwrap_or("").is_empty() {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_request", params.state.as_deref())));
    }
    if params.code_challenge_method.as_deref() != Some("S256") {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_request", params.state.as_deref())));
    }
    if params.state.as_deref().unwrap_or("").is_empty() {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_request", None)));
    }
    if !matches!(params.scope.as_deref(), None | Some("") | Some("mcp")) {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_request", params.state.as_deref())));
    }
    let Some(resource) = params.resource.clone().filter(|s| !s.is_empty()) else {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_target", params.state.as_deref())));
    };
    if resource != root_resource(state) {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_target", params.state.as_deref())));
    }

    Ok((identity, redirect_uri, resource))
}

/// `GET /oauth/authorize` (requirement 3; AC2, AC6).
pub async fn get_authorize(State(state): State<Arc<AppState>>, Query(params): Query<AuthorizeParams>) -> Response {
    let (identity, _redirect_uri, resource) = match validate_authorize_params(&state, &params).await {
        Ok(ok) => ok,
        Err(resp) => return *resp,
    };
    let client_name = identity.client_name.clone().unwrap_or_else(|| identity.client_id.clone());
    html_response(StatusCode::OK, render_consent_page(&client_name, &resource, &params, None))
}

// ---- consent: POST /oauth/authorize ---------------------------------------

/// requirement 3: 60s TTL for a minted authorization code.
const AUTHORIZATION_CODE_TTL_SECS: i64 = 60;

/// `GET`/`POST /oauth/authorize`'s form fields, all of [`AuthorizeParams`]
/// plus the consent page's own proof-of-ownership fields.
#[derive(Debug, Deserialize)]
pub struct ConsentForm {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    pub scope: Option<String>,
    pub resource: Option<String>,
    #[serde(default)]
    pub tenant_key: Option<String>,
    #[serde(default)]
    pub claim_code: Option<String>,
}

fn consent_form_params(form: &ConsentForm) -> AuthorizeParams {
    AuthorizeParams {
        response_type: form.response_type.clone(),
        client_id: form.client_id.clone(),
        redirect_uri: form.redirect_uri.clone(),
        code_challenge: form.code_challenge.clone(),
        code_challenge_method: form.code_challenge_method.clone(),
        state: form.state.clone(),
        scope: form.scope.clone(),
        resource: form.resource.clone(),
    }
}

/// requirement 3: proves tenant ownership either by the raw tenant key or
/// by a claim code from the existing `/claim` flow
/// ([`crate::db::Db::verify_claim_code`], the same single-use, hashed,
/// tenant-tied code the magic-link flow itself redeems) -- `tenant_key`
/// wins if both are somehow present. `None` for a blank/absent/wrong
/// credential either way; the caller never learns which of "wrong" or
/// "absent" it was (same non-distinguishing posture `claim::resolve_claim_token`
/// already takes for its own token lookup).
async fn resolve_consent_tenant(
    state: &AppState,
    tenant_key: Option<&str>,
    claim_code: Option<&str>,
) -> Option<crate::db::Tenant> {
    if let Some(key) = tenant_key.filter(|k| !k.is_empty()) {
        let hash = crate::auth::hash_key(key);
        return state.db.find_tenant_by_key_hash(hash).await.ok().flatten();
    }
    if let Some(code) = claim_code.filter(|c| !c.is_empty()) {
        let hash = crate::auth::hash_key(code);
        if let Ok(crate::db::ClaimVerifyOutcome::Verified { tenant_id, .. }) =
            state.db.verify_claim_code(hash).await
        {
            return state.db.find_tenant_by_id(tenant_id).await.ok().flatten();
        }
    }
    None
}

/// requirement 3/4: mints a single-use authorization code for `tenant` and
/// redirects to `redirect_uri` with `code`/`state` -- the consent POST's
/// one success path.
#[allow(clippy::too_many_arguments)]
async fn mint_code_and_redirect(
    state: &AppState,
    tenant: &crate::db::Tenant,
    identity: &ClientIdentity,
    redirect_uri: &str,
    code_challenge: &str,
    resource: &str,
    state_param: &str,
) -> Response {
    let code = crate::auth::generate_key();
    let code_hash = crate::auth::hash_key(&code);
    let expires_unix = crate::state::now_unix() + AUTHORIZATION_CODE_TTL_SECS;
    let inserted = state
        .db
        .insert_oauth_code(
            code_hash,
            tenant.id,
            identity.client_id.clone(),
            identity.client_name.clone(),
            identity.method.to_string(),
            redirect_uri.to_string(),
            code_challenge.to_string(),
            resource.to_string(),
            "mcp".to_string(),
            expires_unix,
        )
        .await;
    if inserted.is_err() {
        return render_inline_error("server_error", "could not mint an authorization code");
    }
    let Ok(mut url) = reqwest::Url::parse(redirect_uri) else {
        return render_inline_error("invalid_request", "redirect_uri is malformed");
    };
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("code", &code);
        pairs.append_pair("state", state_param);
    }
    Redirect::to(url.as_str()).into_response()
}

/// `POST /oauth/authorize` (requirement 3; AC4, AC6).
pub async fn post_authorize(State(state): State<Arc<AppState>>, axum::Form(form): axum::Form<ConsentForm>) -> Response {
    let params = consent_form_params(&form);
    let (identity, redirect_uri, resource) = match validate_authorize_params(&state, &params).await {
        Ok(ok) => ok,
        Err(resp) => return *resp,
    };
    match resolve_consent_tenant(&state, form.tenant_key.as_deref(), form.claim_code.as_deref()).await {
        Some(tenant) => {
            let code_challenge = params.code_challenge.clone().unwrap_or_default();
            let state_param = params.state.clone().unwrap_or_default();
            mint_code_and_redirect(&state, &tenant, &identity, &redirect_uri, &code_challenge, &resource, &state_param)
                .await
        }
        // requirement 6 / AC6: a tampered consent POST lacking a valid key
        // or claim code re-renders the consent page (never a redirect, and
        // never a minted code) -- the caller gets another chance rather
        // than a hard failure, same posture `claim::post_claim`'s own
        // invalid-email branch takes.
        None => {
            let client_name = identity.client_name.clone().unwrap_or_else(|| identity.client_id.clone());
            html_response(
                StatusCode::OK,
                render_consent_page(
                    &client_name,
                    &resource,
                    &params,
                    Some("that key or claim code did not verify -- try again"),
                ),
            )
        }
    }
}

// ---- DCR: POST /oauth/register --------------------------------------------

/// requirement 2(b): 10 registrations per minute per source IP.
const OAUTH_REGISTER_RATE_LIMIT_PER_MINUTE: i64 = 10;
const OAUTH_REGISTER_RATE_LIMIT_WINDOW_SECS: i64 = 60;

/// Same header-then-peer-fallback shape as `claim.rs`'s own `source_ip`.
fn source_ip(headers: &HeaderMap, peer: SocketAddr) -> String {
    let forwarded_for = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    crate::state::resolve_source_ip(Some(&peer.ip().to_string()), forwarded_for)
}

fn oauth_error_json(status: StatusCode, error: &str, description: &str) -> Response {
    (status, Json(json!({"error": error, "error_description": description}))).into_response()
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    pub application_type: Option<String>,
}

/// requirement 2(b): a `native` app's redirect must be a loopback URL with
/// no port fixed at registration time (`strip_loopback_port` is a no-op on
/// an already-portless URL, so this doubles as "is loopback, host+path
/// only"); any other client's redirects must be `https`.
fn validate_redirect_uris(application_type: &str, redirect_uris: &[String]) -> bool {
    if redirect_uris.is_empty() {
        return false;
    }
    redirect_uris.iter().all(|uri| {
        if application_type == "native" {
            strip_loopback_port(uri).as_deref() == Some(uri.as_str())
        } else {
            uri.starts_with("https://")
        }
    })
}

/// `POST /oauth/register` (requirement 2(b); AC3).
pub async fn post_register(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(req): Json<RegisterRequest>,
) -> Response {
    let ip = source_ip(&headers, peer);
    let since = crate::state::now_unix() - OAUTH_REGISTER_RATE_LIMIT_WINDOW_SECS;
    match state
        .db
        .try_admit_oauth_register(ip, since, OAUTH_REGISTER_RATE_LIMIT_PER_MINUTE)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return oauth_error_json(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                "registration rate limit exceeded for this source; try again later",
            );
        }
        Err(_) => return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "storage error"),
    }

    let application_type = req.application_type.clone().unwrap_or_else(|| "web".to_string());
    if !matches!(application_type.as_str(), "native" | "web") {
        return oauth_error_json(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "application_type must be \"native\" or \"web\"",
        );
    }
    if !validate_redirect_uris(&application_type, &req.redirect_uris) {
        return oauth_error_json(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "redirect_uris is empty, or not valid for this application_type",
        );
    }
    let token_endpoint_auth_method = req.token_endpoint_auth_method.clone().unwrap_or_else(|| {
        if application_type == "native" { "none".to_string() } else { "client_secret_post".to_string() }
    });
    if !matches!(token_endpoint_auth_method.as_str(), "none" | "client_secret_post") {
        return oauth_error_json(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "token_endpoint_auth_method must be \"none\" or \"client_secret_post\"",
        );
    }

    let client_id = format!("dcr_{}", crate::auth::generate_key());
    let (client_secret, client_secret_hash) = if token_endpoint_auth_method == "client_secret_post" {
        let secret = crate::auth::generate_key();
        let hash = crate::auth::hash_key(&secret);
        (Some(secret), Some(hash))
    } else {
        (None, None)
    };
    let Ok(redirect_uris_json) = serde_json::to_string(&req.redirect_uris) else {
        return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not encode redirect_uris");
    };
    match state
        .db
        .create_oauth_client(
            client_id.clone(),
            client_secret_hash,
            req.client_name.clone(),
            redirect_uris_json,
            application_type.clone(),
            token_endpoint_auth_method.clone(),
        )
        .await
    {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not register client");
        }
    }

    let mut body = json!({
        "client_id": client_id,
        "redirect_uris": req.redirect_uris,
        "application_type": application_type,
        "token_endpoint_auth_method": token_endpoint_auth_method,
        "client_id_issued_at": crate::state::now_unix(),
    });
    if let Some(obj) = body.as_object_mut() {
        if let Some(name) = &req.client_name {
            obj.insert("client_name".to_string(), json!(name));
        }
        if let Some(secret) = client_secret {
            obj.insert("client_secret".to_string(), json!(secret));
        }
    }
    (StatusCode::CREATED, Json(body)).into_response()
}

// ---- POST /oauth/token -----------------------------------------------------

/// requirement 5: hosted access-token TTL.
const ACCESS_TOKEN_TTL_SECS: i64 = 3600;
/// requirement 4: refresh-token TTL, 30 days.
const REFRESH_TOKEN_TTL_SECS: i64 = 30 * 86_400;

#[derive(Debug, Default, Deserialize)]
pub struct TokenRequest {
    pub grant_type: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub code_verifier: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

/// RFC 7636 S256: `BASE64URL-ENCODE(SHA256(code_verifier)) == code_challenge`.
fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize()) == challenge
}

/// requirement 4: mints and stores a fresh access/refresh token pair for
/// an already-created-or-rotated grant -- the one success path both
/// `authorization_code` and `refresh_token` grants converge on.
async fn issue_tokens(state: &AppState, grant_id: i64, tenant_id: i64, resource: &str, client_id: &str) -> Response {
    let tenant = match state.db.find_tenant_by_id(tenant_id).await {
        Ok(Some(t)) => t,
        _ => return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "tenant not found"),
    };
    let now = crate::state::now_unix();
    let jti = crate::state::new_ulid();
    let exp = now + ACCESS_TOKEN_TTL_SECS;
    let claims = json!({
        "iss": state.public_url.trim_end_matches('/'),
        "sub": tenant.namespace,
        "aud": resource,
        "scope": "mcp",
        "client_id": client_id,
        "jti": jti,
        "iat": now,
        "exp": exp,
    });
    let access_token = match state.authz_key.sign(&claims) {
        Ok(t) => t,
        Err(_) => return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not sign access token"),
    };
    if state
        .db
        .insert_oauth_jti(jti, grant_id, tenant_id, resource.to_string(), exp)
        .await
        .is_err()
    {
        return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not record access token");
    }
    let refresh_token = crate::auth::generate_key();
    let refresh_hash = crate::auth::hash_key(&refresh_token);
    if state
        .db
        .insert_oauth_refresh_token(grant_id, refresh_hash, resource.to_string(), now + REFRESH_TOKEN_TTL_SECS)
        .await
        .is_err()
    {
        return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not record refresh token");
    }
    let _ = state.db.touch_oauth_grant_last_used(grant_id, now).await;
    (
        StatusCode::OK,
        Json(json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": ACCESS_TOKEN_TTL_SECS,
            "refresh_token": refresh_token,
            "scope": "mcp",
        })),
    )
        .into_response()
}

/// requirement 4/5, AC4/AC5/AC7: `grant_type=authorization_code`.
async fn token_authorization_code(state: &AppState, req: &TokenRequest) -> Response {
    let Some(code) = req.code.as_deref().filter(|s| !s.is_empty()) else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_request", "code is required");
    };
    let Some(client_id) = req.client_id.as_deref().filter(|s| !s.is_empty()) else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_request", "client_id is required");
    };
    let code_hash = crate::auth::hash_key(code);
    let Ok(Some(row)) = state.db.find_oauth_code_by_hash(code_hash.clone()).await else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "unknown code");
    };
    let now = crate::state::now_unix();

    // AC5/AC7: a replayed code -- revoke whatever it minted the first
    // time (if it minted anything at all; a race with another concurrent
    // redemption attempt could have consumed it with no grant recorded
    // yet) and refuse, regardless of whether every other field still
    // matches.
    if row.consumed_unix.is_some() {
        if let Some(grant_id) = row.grant_id {
            let _ = state.db.revoke_oauth_grant(grant_id, now).await;
            let _ = state.db.deny_oauth_jtis_for_grant(grant_id, now).await;
            let _ = state.db.revoke_oauth_refresh_tokens_for_grant(grant_id, now).await;
        }
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "code already used");
    }
    if now > row.expires_unix {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "code expired");
    }
    // AC7: client A's code presented by client B.
    if row.client_id != client_id {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "client_id does not match this code");
    }
    if req.redirect_uri.as_deref() != Some(row.redirect_uri.as_str()) {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "redirect_uri does not match this code");
    }
    if let Some(resource) = req.resource.as_deref()
        && resource != row.resource
    {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "resource does not match this code");
    }
    let Some(verifier) = req.code_verifier.as_deref().filter(|s| !s.is_empty()) else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "code_verifier is required");
    };
    if !pkce_matches(verifier, &row.code_challenge) {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "code_verifier does not match code_challenge");
    }
    let Some(identity) = identify_client(state, client_id).await else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_client", "unknown client");
    };
    if let Some(secret_hash) = &identity.client_secret_hash {
        let Some(secret) = req.client_secret.as_deref().filter(|s| !s.is_empty()) else {
            return oauth_error_json(StatusCode::UNAUTHORIZED, "invalid_client", "client_secret is required");
        };
        if &crate::auth::hash_key(secret) != secret_hash {
            return oauth_error_json(StatusCode::UNAUTHORIZED, "invalid_client", "client_secret is incorrect");
        }
    }

    // The atomic single-use claim -- everything above was a read-only
    // check; this is the one statement that actually consumes the code,
    // so a second request racing this one (both having passed every check
    // above) can still only mint tokens once.
    if !state.db.claim_oauth_code(code_hash.clone(), now).await.unwrap_or(false) {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "code already used");
    }
    let grant_id = match state
        .db
        .create_oauth_grant(row.tenant_id, row.client_id.clone(), row.client_name.clone(), row.method.clone(), row.resource.clone())
        .await
    {
        Ok(id) => id,
        Err(_) => return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not create grant"),
    };
    let _ = state.db.set_oauth_code_grant(code_hash, grant_id).await;
    issue_tokens(state, grant_id, row.tenant_id, &row.resource, client_id).await
}

/// requirement 4, AC7: `grant_type=refresh_token` -- rotates the refresh
/// token; reuse of an already-rotated one revokes the whole grant.
async fn token_refresh_token(state: &AppState, req: &TokenRequest) -> Response {
    let Some(token) = req.refresh_token.as_deref().filter(|s| !s.is_empty()) else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_request", "refresh_token is required");
    };
    let hash = crate::auth::hash_key(token);
    let Ok(Some(row)) = state.db.find_oauth_refresh_token_by_hash(hash.clone()).await else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "unknown refresh token");
    };
    let now = crate::state::now_unix();
    if row.rotated_unix.is_some() {
        let _ = state.db.revoke_oauth_grant(row.grant_id, now).await;
        let _ = state.db.deny_oauth_jtis_for_grant(row.grant_id, now).await;
        let _ = state.db.revoke_oauth_refresh_tokens_for_grant(row.grant_id, now).await;
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "refresh token already rotated");
    }
    if now > row.expires_unix {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "refresh token expired");
    }
    let Ok(Some(grant)) = state.db.find_oauth_grant_by_id(row.grant_id).await else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "grant not found");
    };
    if grant.revoked_unix.is_some() {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "grant has been revoked");
    }
    if !state.db.claim_oauth_refresh_token(hash, now).await.unwrap_or(false) {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "refresh token already rotated");
    }
    issue_tokens(state, row.grant_id, grant.tenant_id, &row.resource, &grant.client_id).await
}

/// `POST /oauth/token` (requirement 4; AC4, AC5, AC7).
pub async fn post_token(State(state): State<Arc<AppState>>, axum::Form(req): axum::Form<TokenRequest>) -> Response {
    match req.grant_type.as_deref() {
        Some("authorization_code") => token_authorization_code(&state, &req).await,
        Some("refresh_token") => token_refresh_token(&state, &req).await,
        _ => oauth_error_json(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "grant_type must be \"authorization_code\" or \"refresh_token\"",
        ),
    }
}

// ---- hosted-token bearer validation ---------------------------------------

/// requirement 5: same clock-skew allowance `oauth.rs`'s own bring-your-own-
/// issuer validation uses.
const CLOCK_SKEW_SECS: i64 = 60;

/// requirement 5: validates a bearer JWT whose `iss` is this host's own
/// `public_url` -- called from [`crate::oauth::validate_bearer`], which
/// peeks `iss` first and routes here instead of the tenant-registered-issuer
/// path. `sub` is the tenant's own `namespace` (this host's only concept of
/// a "tenant public id"); `aud` must be the root resource URI (per-tenant
/// resources aren't live yet); a `jti` absent from `oauth_jti_denylist`
/// (this crate never mints one without recording it) or found `denied_unix`
/// both refuse as `"revoked"`.
pub async fn validate_hosted_bearer(
    state: &AppState,
    token: &str,
    header: &jsonwebtoken::Header,
) -> Result<crate::oauth::OauthCaller, AppError> {
    if header.kid.as_deref() != Some(state.authz_key.kid.as_str()) {
        return Err(AppError::InvalidToken("bad_signature"));
    }
    let decoding_key = state.authz_key.decoding_key()?;
    let mut validation = jsonwebtoken::Validation::new(Algorithm::ES256);
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    let token_data = jsonwebtoken::decode::<Value>(token, &decoding_key, &validation)
        .map_err(|_| AppError::InvalidToken("bad_signature"))?;
    let claims = token_data.claims;

    let now = crate::state::now_unix();
    let exp = claims.get("exp").and_then(Value::as_i64).ok_or(AppError::InvalidToken("malformed"))?;
    if now > exp + CLOCK_SKEW_SECS {
        return Err(AppError::InvalidToken("expired"));
    }
    let aud = claims.get("aud").and_then(Value::as_str).ok_or(AppError::InvalidToken("malformed"))?;
    if aud != root_resource(state) {
        return Err(AppError::InvalidToken("wrong_audience"));
    }
    let sub = claims
        .get("sub")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(AppError::InvalidToken("malformed"))?;
    let jti = claims
        .get("jti")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(AppError::InvalidToken("malformed"))?;

    let jti_row = state
        .db
        .find_oauth_jti(jti)
        .await?
        .ok_or(AppError::InvalidToken("revoked"))?;
    if jti_row.denied_unix.is_some() {
        return Err(AppError::InvalidToken("revoked"));
    }
    let tenant = state
        .db
        .find_tenant_by_namespace(sub)
        .await?
        .ok_or(AppError::InvalidToken("malformed"))?;
    let _ = state.db.touch_oauth_grant_last_used(jti_row.grant_id, now).await;

    Ok(crate::oauth::OauthCaller {
        tenant_id: tenant.id,
        subject: tenant.namespace,
        issuer: state.public_url.trim_end_matches('/').to_string(),
        auth_method: "hosted_token",
    })
}

// ---- host.oauth.grants / host.oauth.grant_revoke / POST /oauth/revoke ----

/// `host.oauth.grants` (requirement 7; AC8): this tenant's live grants.
pub async fn grants_list(state: &AppState, tenant: &crate::db::Tenant) -> Result<Value, AppError> {
    let rows = state.db.list_oauth_grants_by_tenant(tenant.id).await?;
    let grants: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "client_id": r.client_id,
                "client_name": r.client_name,
                "method": r.method,
                "resource": r.resource,
                "created_at": r.created_unix,
                "last_used_at": r.last_used_unix,
            })
        })
        .collect();
    Ok(json!({"grants": grants}))
}

/// `host.oauth.grant_revoke {id}` (requirement 7; AC8): revokes a grant
/// this tenant owns -- its access tokens fail within 60s (jti denylist,
/// checked on every hosted-bearer call) and its refresh tokens stop
/// rotating. Recorded to `admin_audit` with `actor_key_id` this tenant's
/// own `key_hash` (already a hash, never the raw key -- AC9) since this is
/// a tenant-initiated mutation, not an admin one.
pub async fn grant_revoke(state: &AppState, tenant: &crate::db::Tenant, args: &Value) -> Result<Value, AppError> {
    let id = args
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'id'".to_string()))?;
    let now = crate::state::now_unix();
    let revoked = state.db.revoke_oauth_grant_for_tenant(id, tenant.id, now).await?;
    if !revoked {
        return Err(AppError::Structured {
            code: "grant_not_found",
            message: format!("no live grant {id} for this tenant"),
            data: json!({"id": id}),
        });
    }
    let _ = state.db.deny_oauth_jtis_for_grant(id, now).await;
    let _ = state.db.revoke_oauth_refresh_tokens_for_grant(id, now).await;
    let _ = state
        .db
        .record_admin_audit(tenant.key_hash.clone(), "oauth_grant_revoke".to_string(), Some(id.to_string()), None)
        .await;
    Ok(json!({"revoked": true, "id": id}))
}

#[derive(Deserialize)]
pub struct RevokeRequest {
    pub token: Option<String>,
    #[serde(default)]
    pub token_type_hint: Option<String>,
}

/// Hand-written rather than derived: `token` is the raw refresh/access
/// token itself (RFC 7009 `POST /oauth/revoke` body), unlike
/// `UpstreamAuthSpec::secret` above which is only ever a reference name --
/// so `{:?}` must never print it. Redact unconditionally.
impl std::fmt::Debug for RevokeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevokeRequest")
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .field("token_type_hint", &self.token_type_hint)
            .finish()
    }
}

/// `POST /oauth/revoke` (RFC 7009; requirement 7, AC8): revokes the grant
/// a refresh token belongs to. Always 200 regardless of whether `token`
/// was a live refresh token -- RFC 7009 §2.2's own posture (never letting
/// the response distinguish "unknown token" from "revoked"), and there is
/// no tenant credential on this route to authenticate any other way:
/// possessing the refresh token itself is the proof.
pub async fn post_revoke(State(state): State<Arc<AppState>>, axum::Form(req): axum::Form<RevokeRequest>) -> Response {
    if let Some(token) = req.token.as_deref().filter(|s| !s.is_empty()) {
        let hash = crate::auth::hash_key(token);
        if let Ok(Some(row)) = state.db.find_oauth_refresh_token_by_hash(hash).await {
            let now = crate::state::now_unix();
            let _ = state.db.revoke_oauth_grant(row.grant_id, now).await;
            let _ = state.db.deny_oauth_jtis_for_grant(row.grant_id, now).await;
            let _ = state.db.revoke_oauth_refresh_tokens_for_grant(row.grant_id, now).await;
        }
    }
    StatusCode::OK.into_response()
}
