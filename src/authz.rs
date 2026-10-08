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
use serde_json::{Map, Value, json};
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

/// PRD-mcphost-federated-end-user-login requirement 2: a tenant's own
/// canonical per-tenant resource URI -- minimal, additive support for the
/// one shape this PRD needs (`/t/<ns>/mcp`, served identically to `/mcp`
/// by `http.rs`'s own second route registration); full per-tenant
/// metadata/audience enforcement is PRD-mcphost-tenant-resource-metadata's
/// job, not this one's.
pub fn per_tenant_resource(state: &AppState, namespace: &str) -> String {
    format!("{}/t/{}/mcp", state.public_url.trim_end_matches('/'), namespace)
}

/// The inverse of [`per_tenant_resource`]: `<ns>` from a `.../t/<ns>/mcp`
/// resource URI, or `None` for anything else (including the root resource).
/// `main`'s own resource resolution (`resolve_resource_tenant`, below) uses
/// `crate::oauth::resource_tenant_namespace` instead (parses against
/// `public_url` directly, no `AppState` needed there); this `AppState`-taking
/// twin is PRD-mcphost-enterprise-managed-auth requirement 3's own: the
/// JWT-bearer grant handler in `assertion.rs` resolves an assertion's
/// `resource` target the same way (AC4's `resource = <public>/t/nope/mcp`
/// for an unknown `<ns>` is `invalid_target` the same way a malformed
/// resource string is, both resolved by the caller finding no tenant for
/// whatever this returns).
pub fn resource_tenant_namespace<'a>(state: &AppState, resource: &'a str) -> Option<&'a str> {
    let prefix = format!("{}/t/", state.public_url.trim_end_matches('/'));
    resource.strip_prefix(&prefix)?.strip_suffix("/mcp").filter(|ns| !ns.is_empty())
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
        // PRD-mcphost-enterprise-managed-auth requirement 2 (AC5): the
        // JWT-bearer grant is advertised on both the root and per-tenant
        // metadata documents (see [`authorization_server_metadata_for_tenant`]).
        "grant_types_supported": [
            "authorization_code",
            "refresh_token",
            crate::assertion::JWT_BEARER_GRANT_TYPE,
        ],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_post"],
        "scopes_supported": ["mcp"],
        "client_id_metadata_document_supported": true,
    })
}

/// PRD-mcphost-enterprise-managed-auth requirement 2 (AC5): the per-tenant
/// twin of [`authorization_server_metadata`] above -- identical fields,
/// plus `identity_assertion_issuers_supported` naming this one tenant's
/// own trusted identity-assertion issuers (never another tenant's). The
/// draft (draft-ietf-oauth-identity-assertion-authz-grant-04) does not
/// actually define a metadata field name for this at all (verified at
/// build time; see the intent card's `ambiguities_resolved`) -- this uses
/// the name this PRD's own requirement 2 specifies.
pub async fn authorization_server_metadata_for_tenant(
    state: &AppState,
    tenant: &crate::db::Tenant,
) -> Result<Value, AppError> {
    let mut doc = authorization_server_metadata(state);
    let issuers: Vec<String> =
        state.db.list_trusted_issuers_by_tenant(tenant.id).await?.into_iter().map(|row| row.issuer).collect();
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("identity_assertion_issuers_supported".to_string(), json!(issuers));
    }
    Ok(doc)
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
        // PRD-mcphost-oauth-demand-signal requirement 3: this CIMD
        // client's first-ever identification -- `INSERT OR IGNORE`d, so a
        // repeat resolution (a second authorize request, or the token
        // exchange re-identifying the same client) never moves
        // `created_unix` off this client's true first-seen time.
        // Best-effort: a write failure here must never fail authorization.
        let _ = state
            .db
            .record_cimd_client_seen(client_id.to_string(), crate::state::now_unix())
            .await;
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

/// PRD-mcphost-tool-scopes-and-consent requirement 2 (AC1): one `<div
/// class="scope" data-scope="{name}">` block per space-separated word in
/// `resolved_scope` (skipping `mcp`/`offline_access`, which have no catalog
/// concept) -- each names its own catalog description (when this tenant set
/// one via `host.oauth.scope_set`) and, in a nested `<ul>`, every one of
/// this tenant's own tools whose `scopes` includes that word. Resolved
/// against `resource`'s own tenant *before* consent is proven (the `resource`
/// parameter itself already names it -- see [`resolve_resource_tenant`]);
/// `resource_tenant: None` (the root resource) renders nothing, since `mcp`
/// is the only scope the root resource ever carries.
async fn scope_groups_html(state: &AppState, resource_tenant: Option<&crate::db::Tenant>, resolved_scope: &str) -> String {
    let Some(tenant) = resource_tenant else {
        return String::new();
    };
    let catalog = state.db.list_oauth_scopes(tenant.id).await.unwrap_or_default();
    let tools = state.db.list_tools(tenant.id).await.unwrap_or_default();
    let mut out = String::new();
    for word in resolved_scope.split_whitespace() {
        if word == "mcp" || word == "offline_access" {
            continue;
        }
        let description = catalog.iter().find(|s| s.name == word).map(|s| s.description.as_str());
        let desc_html = description
            .map(|d| format!(" &mdash; {}", html_escape(d)))
            .unwrap_or_default();
        let items: String = tools
            .iter()
            .filter(|t| t.scopes.iter().any(|s| s == word))
            .map(|t| format!("<li class=\"tool\">{}</li>", html_escape(&t.name)))
            .collect();
        out.push_str(&format!(
            "<div class=\"scope\" data-scope=\"{name}\"><h3>{name}{desc_html}</h3><ul>{items}</ul></div>",
            name = html_escape(word),
        ));
    }
    out
}

/// requirement (P1)/AC7: the unverified-application caution links here --
/// a short, static explainer served by [`get_unverified_app_explainer`],
/// no tenant/client context needed.
const UNVERIFIED_APP_EXPLAINER_PATH: &str = "/oauth/unverified-app";

/// AC5: `identity.client_name`, trimmed and treated as absent when blank --
/// an empty-but-present `client_name` (a DCR client can register one) must
/// never render as an empty `<h1>`.
fn display_client_name(identity: &ClientIdentity) -> &str {
    identity
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("(unnamed application)")
}

/// requirement (P0)/AC3: the host component of the `redirect_uri` the
/// authorization code would be delivered to -- `redirect_uri` is already
/// `redirect_uri_allowed`-validated by the time either consent-page call
/// site renders, so this only ever fails to parse on a malformed value that
/// validation would already have refused; the fallback string is display
/// text only; it grants nothing.
fn destination_host(redirect_uri: &str) -> String {
    reqwest::Url::parse(redirect_uri)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "(unknown destination)".to_string())
}

/// requirement (P0)/AC1/AC4/AC6: `true` iff this client's consent page must
/// carry the unverified-application caution -- a DCR client not on the
/// operator's [`crate::state::VerifiedClientIds`] allowlist. A CIMD client
/// (its identity is anchored to a fetched, validated metadata document) is
/// never unverified (AC2).
fn is_unverified(identity: &ClientIdentity, verified_client_ids: &crate::state::VerifiedClientIds) -> bool {
    identity.method == "dcr" && !verified_client_ids.contains(&identity.client_id)
}

/// AC1/AC7: the caution block itself -- states plainly that the app
/// self-registered and is not operator-verified, and links to
/// [`UNVERIFIED_APP_EXPLAINER_PATH`].
fn unverified_caution_html() -> String {
    format!(
        "<div class=\"caution\"><p><strong>Unverified application.</strong> \
         This app registered itself with mcphost through open client \
         registration and has not been verified by mcphost's operator -- \
         the name below is only what the app calls itself. \
         <a href=\"{path}\">What does this mean?</a></p></div>",
        path = UNVERIFIED_APP_EXPLAINER_PATH,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_consent_page(
    identity: &ClientIdentity,
    verified_client_ids: &crate::state::VerifiedClientIds,
    resource: &str,
    params: &AuthorizeParams,
    resolved_scope: &str,
    scope_groups: &str,
    error: Option<&str>,
    offer_new_workspace: bool,
) -> String {
    // PRD-mcphost-oauth-first-grant-signup requirement 1: the third form,
    // only where a brand-new tenant could actually own the grant (never a
    // per-tenant resource, whose consent must prove THAT tenant).
    let new_workspace_html = if offer_new_workspace {
        "<p>...or, if you have no workspace yet:</p>\
         <button type=\"submit\" name=\"new_workspace\" value=\"1\">Create a new workspace</button>"
    } else {
        ""
    };
    let error_html = error
        .map(|e| format!("<p class=\"err\">{}</p>", html_escape(e)))
        .unwrap_or_default();
    // AC4: a DCR client's `client_name` is self-asserted -- escaped like
    // any other, but never rendered as if it were verified/first-party.
    let is_dcr = identity.method == "dcr";
    let client = if is_dcr {
        format!(
            "{} <span class=\"self-asserted\">(self-reported name)</span>",
            html_escape(display_client_name(identity))
        )
    } else {
        html_escape(display_client_name(identity))
    };
    let caution_html = if is_unverified(identity, verified_client_ids) {
        unverified_caution_html()
    } else {
        String::new()
    };
    // AC3: shown for every client, DCR or CIMD alike.
    let destination = html_escape(&destination_host(opt_str(&params.redirect_uri)));
    page(
        "mcphost — authorize",
        &format!(
            "<h1>Authorize {client}</h1>\
             {caution_html}\
             <p>{client} is requesting access to <code>{resource}</code> on your behalf.</p>\
             <p>Your authorization will be sent to <strong>{destination}</strong>.</p>\
             {scope_groups}\
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
             {new_workspace_html}\
             </form>",
            resource = html_escape(resource),
            response_type = html_escape(opt_str(&params.response_type)),
            client_id = html_escape(opt_str(&params.client_id)),
            redirect_uri = html_escape(opt_str(&params.redirect_uri)),
            code_challenge = html_escape(opt_str(&params.code_challenge)),
            code_challenge_method = html_escape(opt_str(&params.code_challenge_method)),
            state = html_escape(opt_str(&params.state)),
            scope = html_escape(resolved_scope),
            resource_attr = html_escape(opt_str(&params.resource)),
        ),
    )
}

/// AC7: a short, static explainer of what an "unverified application"
/// caution means -- no tenant/client context, so no auth or lookup needed.
pub async fn get_unverified_app_explainer() -> Response {
    html_response(
        StatusCode::OK,
        page(
            "mcphost — unverified application",
            "<h1>What does \"unverified application\" mean?</h1>\
             <p>mcphost lets any application register itself as an OAuth \
             client with no operator involvement (open dynamic client \
             registration). A self-registered application chooses its own \
             display name and picks its own redirect destination -- mcphost \
             has not confirmed who operates it or that its name is accurate.</p>\
             <p>Before approving, check that the application's name and the \
             destination host shown on the consent page are ones you \
             recognize and trust. Approving sends your authorization to that \
             destination, whatever the application called itself.</p>",
        ),
    )
}

/// PRD-mcphost-oauth-client-policy requirement 2 (AC2): `clients: approve`
/// holds consent for a not-yet-decided client -- rendered instead of
/// minting a code, same "inline, no redirect" posture
/// [`render_inline_error`] takes (the caller has no token to receive yet
/// either way).
fn render_pending_page(client_name: &str) -> String {
    page(
        "mcphost — authorize",
        &format!(
            "<h1>Awaiting approval</h1><p>{} is awaiting approval by this tenant. \
             An admin must call host.oauth.client_approve before this client can connect.</p>",
            html_escape(client_name),
        ),
    )
}

/// requirement 3/AC2/AC6: every failure that has no trusted `redirect_uri`
/// to send the caller back to yet (client identification itself failed, or
/// `redirect_uri` isn't a registered one) renders inline -- never a
/// redirect, since redirecting to an unvalidated URI is exactly the
/// open-redirect flaw class this PRD's grounding names.
fn render_inline_error(code: &str, message: &str) -> Response {
    render_inline_error_status(StatusCode::BAD_REQUEST, code, message)
}

/// Same shape as [`render_inline_error`], with the status code callable --
/// requirement 6 / AC8's rate-limit refusal is a `429`, not this module's
/// usual `400`.
fn render_inline_error_status(status: StatusCode, code: &str, message: &str) -> Response {
    html_response(
        status,
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

/// PRD-mcphost-tool-scopes-and-consent requirement 2: `resource`'s own
/// tenant, resolved before any credential is proven (the URL itself
/// already names it, same as `/t/{ns}/mcp` does for a resource-server
/// call) -- `Ok(None)` for the root resource (no tenant, `mcp`-only, the
/// behaviour every pre-existing test pins), `Ok(Some(tenant))` for a
/// recognized per-tenant resource, `Err(())` for anything else (an
/// unrecognized `resource`, or one naming a tenant that doesn't exist).
async fn resolve_resource_tenant(state: &AppState, resource: &str) -> Result<Option<crate::db::Tenant>, ()> {
    if resource == root_resource(state) {
        return Ok(None);
    }
    let Some(ns) = crate::oauth::resource_tenant_namespace(&state.public_url, resource) else {
        return Err(());
    };
    match state.db.find_tenant_by_namespace(ns).await {
        Ok(Some(tenant)) => Ok(Some(tenant)),
        _ => Err(()),
    }
}

/// requirement 2: every scope name valid to request against `resource` --
/// `mcp` alone for the root resource (unchanged), `mcp` plus the built-ins
/// and this tenant's own catalog for a per-tenant resource. Never includes
/// `offline_access`, checked separately (always allowed, requirement 2).
async fn known_request_scopes(
    state: &AppState,
    resource_tenant: Option<&crate::db::Tenant>,
) -> std::collections::HashSet<String> {
    match resource_tenant {
        None => std::iter::once("mcp".to_string()).collect(),
        Some(tenant) => {
            let catalog = state.db.list_oauth_scopes(tenant.id).await.unwrap_or_default();
            crate::oauth::known_scope_names(&catalog)
        }
    }
}

/// requirement 2: validates the requested `scope` string against
/// `known`, normalizing an absent/blank request to `"mcp"` (the pre-existing
/// behaviour every earlier test pins) -- `Ok` is the resolved,
/// single-space-joined scope string the code/token will actually carry.
fn resolve_and_validate_scope(raw: Option<&str>, known: &std::collections::HashSet<String>) -> Option<String> {
    let words: Vec<&str> = raw.unwrap_or("").split_whitespace().collect();
    if words.is_empty() {
        return Some("mcp".to_string());
    }
    for w in &words {
        if *w != "offline_access" && !known.contains(*w) {
            return None;
        }
    }
    Some(words.join(" "))
}

/// requirement 3/AC2/AC6: every validation `GET`/`POST /oauth/authorize`
/// share -- client identification, redirect-uri registration, then (once
/// `redirect_uri` is trusted) PKCE/state/scope/resource. `Ok` carries the
/// identified client, the now-trusted `redirect_uri`, the validated
/// `resource`, the resolved `scope` string, and `resource`'s own tenant (for
/// the consent page's per-scope tool listing, `None` for the root
/// resource); `Err` is an already-built response (inline for the first two
/// failure classes, a redirect to the now-trusted `redirect_uri` for the
/// rest) -- requirement 6: "consent cannot be skipped by any parameter", so
/// `POST /oauth/authorize` runs this exact same function over its own
/// (re-submitted, not blindly trusted) fields rather than trusting whatever
/// the hidden form fields said.
#[allow(clippy::type_complexity)]
async fn validate_authorize_params(
    state: &AppState,
    params: &AuthorizeParams,
    source_ip: &str,
) -> Result<(ClientIdentity, String, String, String, Option<crate::db::Tenant>), Box<Response>> {
    let Some(client_id) = params.client_id.clone().filter(|s| !s.is_empty()) else {
        return Err(Box::new(render_inline_error("invalid_request", "client_id is required")));
    };
    let Some(identity) = identify_client(state, &client_id).await else {
        return Err(Box::new(render_inline_error(
            "invalid_request",
            "client_id could not be identified (unknown, or its CIMD document did not validate)",
        )));
    };
    // PRD-mcphost-oauth-client-policy requirement 5 (AC7): the operator's
    // global block list is checked before any tenant policy, and before
    // this client can even be identified as belonging to one -- no tenant
    // is known yet at this point, so the audit row this writes carries no
    // `tenant_id`.
    if crate::oauth_policy::is_client_blocked(state, &identity).await.unwrap_or(false) {
        let _ = crate::oauth_policy::record_audit(
            state,
            None,
            "authorize_refused",
            Some(identity.client_id.as_str()),
            Some(identity.method),
            None,
            Some("client_blocked"),
            Some(source_ip),
        )
        .await;
        return Err(Box::new(render_inline_error("client_blocked", "this client is blocked by the operator")));
    }
    // requirement 6 (AC8): a CIMD client's first sighting each day counts
    // against its host's 100-per-day cap -- checked here, the one place
    // every CIMD authorize attempt (GET and POST alike) passes through.
    if !crate::oauth_policy::admit_cimd_registration(state, &identity).await.unwrap_or(true) {
        let _ = crate::oauth_policy::record_audit(
            state,
            None,
            "authorize_refused",
            Some(identity.client_id.as_str()),
            Some(identity.method),
            None,
            Some("cimd_registration_rate_limited"),
            Some(source_ip),
        )
        .await;
        return Err(Box::new(render_inline_error_status(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            "this CIMD host has exceeded its daily registration limit",
        )));
    }
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
    let Some(resource) = params.resource.clone().filter(|s| !s.is_empty()) else {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_target", params.state.as_deref())));
    };
    // PRD-mcphost-federated-end-user-login requirement 2/AC7: the root
    // resource keeps its pre-existing "any tenant's key/claim" consent
    // (`resource_tenant: None`); a per-tenant resource resolves to exactly
    // one tenant, or `invalid_target` if its namespace doesn't exist.
    let Ok(resource_tenant) = resolve_resource_tenant(state, &resource).await else {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_target", params.state.as_deref())));
    };
    let known = known_request_scopes(state, resource_tenant.as_ref()).await;
    let Some(resolved_scope) = resolve_and_validate_scope(params.scope.as_deref(), &known) else {
        return Err(Box::new(error_redirect(&redirect_uri, "invalid_request", params.state.as_deref())));
    };

    Ok((identity, redirect_uri, resource, resolved_scope, resource_tenant))
}

/// `GET /oauth/authorize` (requirement 3; AC2, AC6).
///
/// PRD-mcphost-federated-end-user-login requirement 2/4 (AC2, AC7): a
/// per-tenant resource whose tenant has a provider with `owner_login:
/// false` (the default) redirects straight upstream -- no local page is
/// ever rendered, so the key/claim form is entirely absent (AC7).
pub async fn get_authorize(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuthorizeParams>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let ip = source_ip(&headers, peer);
    // PRD-mcphost-oauth-demand-signal requirement 3 (AC4): every hit to
    // this endpoint counts toward `authorize_requests_7d` -- before
    // validation, so the funnel shows the true drop-off between "showed
    // up" and "consented" rather than only the requests that already
    // passed every check. Best-effort: never fails the request that
    // carries it.
    let funnel_origin = crate::state::classify_funnel_origin(&ip, false, &state.fleet_ips);
    let _ = state.db.record_oauth_funnel_event("authorize_request", funnel_origin).await;
    let (identity, redirect_uri, resource, resolved_scope, resource_tenant) =
        match validate_authorize_params(&state, &params, &ip).await {
            Ok(ok) => ok,
            Err(resp) => return *resp,
        };
    if let Some(tenant) = &resource_tenant
        && let Ok(Some(provider)) = state.db.find_oauth_provider_by_tenant(tenant.id).await
        && !provider.owner_login
    {
        return crate::federation::start(&state, tenant, &provider, &identity, &redirect_uri, &resource, &params).await;
    }
    let scope_groups = scope_groups_html(&state, resource_tenant.as_ref(), &resolved_scope).await;
    html_response(
        StatusCode::OK,
        render_consent_page(
            &identity,
            &state.verified_client_ids,
            &resource,
            &params,
            &resolved_scope,
            &scope_groups,
            None,
            resource_tenant.is_none(),
        ),
    )
}

// ---- consent: POST /oauth/authorize ---------------------------------------

/// requirement 3: 60s TTL for a minted authorization code.
pub(crate) const AUTHORIZATION_CODE_TTL_SECS: i64 = 60;

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
    /// PRD-mcphost-oauth-first-grant-signup requirement 1: the "Create a
    /// new workspace" submit button's own value.
    #[serde(default)]
    pub new_workspace: Option<String>,
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

/// PRD-mcphost-oauth-first-grant-signup requirement 2/3: mints the tenant
/// through [`crate::control::signup`] -- the same limiter, fleet-IP
/// classification, ban check and pause file implicit signup uses -- and
/// renders its refusal on the consent page with the same error code.
async fn create_first_grant_tenant(
    state: &AppState,
    headers: &HeaderMap,
    ip: &str,
) -> Result<crate::db::Tenant, Box<Response>> {
    let ulid = crate::state::new_ulid();
    let name = format!("agent-{}", ulid[ulid.len() - 8..].to_lowercase());
    let user_agent = headers.get("user-agent").and_then(|v| v.to_str().ok());
    let signup = crate::control::signup(
        state,
        &json!({"name": name, "source": "oauth"}),
        ip,
        crate::control::SignupAttribution {
            synthetic_header: None,
            client_name: None,
            client_version: None,
            user_agent,
            origin_header: None,
        },
    )
    .await;
    let refusal = |err: AppError| {
        let err = match err {
            AppError::RateLimited => {
                AppError::signup_rate_limited(&state.public_url, crate::state::SIGNUP_RATE_LIMIT_WINDOW_SECS)
            }
            other => other,
        };
        let status = match err.code() {
            "signup_rate_limited" | "signup_paused" => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::BAD_REQUEST,
        };
        Box::new(render_inline_error_status(status, err.code(), &err.to_string()))
    };
    let value = signup.map_err(refusal)?;
    let key = value
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| Box::new(render_inline_error("server_error", "could not create a workspace")))?;
    match state.db.find_tenant_by_key_hash(crate::auth::hash_key(key)).await {
        Ok(Some(tenant)) => Ok(tenant),
        _ => Err(Box::new(render_inline_error("server_error", "could not create a workspace"))),
    }
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
    scope: &str,
    state_param: &str,
    funnel_origin: &str,
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
            scope.to_string(),
            expires_unix,
            None,
        )
        .await;
    if inserted.is_err() {
        return render_inline_error("server_error", "could not mint an authorization code");
    }
    // PRD-mcphost-oauth-demand-signal requirement 3 (AC4): a code minted
    // here IS consent granted -- `admin.oauth.demand_stats`'s `consents_7d`.
    // Best-effort: never fails the redirect that carries the real code.
    let _ = state.db.record_oauth_funnel_event("consent", funnel_origin).await;
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
pub async fn post_authorize(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    axum::Form(form): axum::Form<ConsentForm>,
) -> Response {
    let ip = source_ip(&headers, peer);
    // PRD-mcphost-oauth-demand-signal requirement 3 (AC4): same
    // count-every-hit rationale as `get_authorize`'s own copy of this line
    // -- the consent-page submission is its own distinct authorize request.
    let funnel_origin = crate::state::classify_funnel_origin(&ip, false, &state.fleet_ips);
    let _ = state.db.record_oauth_funnel_event("authorize_request", funnel_origin).await;
    let params = consent_form_params(&form);
    let (identity, redirect_uri, resource, resolved_scope, resource_tenant) =
        match validate_authorize_params(&state, &params, &ip).await {
            Ok(ok) => ok,
            Err(resp) => return *resp,
        };
    // PRD-mcphost-federated-end-user-login requirement 2/4: defense in
    // depth -- the key/claim form is never rendered for this resource
    // (`get_authorize` above), so a legitimate browser never submits this,
    // but a direct POST must still be refused rather than silently
    // authenticating a tenant key against a federation-only resource.
    if let Some(tenant) = &resource_tenant
        && let Ok(Some(provider)) = state.db.find_oauth_provider_by_tenant(tenant.id).await
        && !provider.owner_login
    {
        return render_inline_error(
            "invalid_request",
            "this tenant's resource requires federated login, not a tenant key",
        );
    }
    // PRD-mcphost-oauth-first-grant-signup requirement 2: neither credential
    // posted + the new-workspace button + a host-wide resource (a per-tenant
    // resource must prove THAT tenant) mints a fresh tenant for this grant.
    let blank = |v: &Option<String>| v.as_deref().is_none_or(str::is_empty);
    let first_grant_signup = !blank(&form.new_workspace)
        && blank(&form.tenant_key)
        && blank(&form.claim_code)
        && resource_tenant.is_none();
    let proven_tenant = if first_grant_signup {
        match create_first_grant_tenant(&state, &headers, &ip).await {
            Ok(tenant) => Some(tenant),
            Err(resp) => return *resp,
        }
    } else {
        resolve_consent_tenant(&state, form.tenant_key.as_deref(), form.claim_code.as_deref()).await
    };
    // PRD-mcphost-tool-scopes-and-consent requirement 2: for a per-tenant
    // resource, the proof of ownership must be for THAT tenant -- another
    // tenant's own valid key proves nothing about `resource`'s catalog.
    let proven_tenant = proven_tenant.filter(|t| resource_tenant.as_ref().is_none_or(|rt| rt.id == t.id));
    match proven_tenant {
        Some(tenant) => {
            // PRD-mcphost-oauth-client-policy requirement 2 (AC1/AC2): the
            // tenant is only known once ownership is proven right here, so
            // this is the one point `allowlist`/`approve` enforcement can
            // actually run.
            let decision = crate::oauth_policy::evaluate_client(&state, &tenant, &identity).await;
            match decision {
                Ok(crate::oauth_policy::ClientDecision::Allowed) => {
                    let code_challenge = params.code_challenge.clone().unwrap_or_default();
                    let state_param = params.state.clone().unwrap_or_default();
                    // requirement (P2)/AC8: note the client's unverified
                    // status on the audit row at the moment consent is
                    // actually granted, so an operator's auth audit can
                    // later see it was unverified when approved -- `None`
                    // (unchanged) for a CIMD or allowlisted client (AC9).
                    let consent_reason = is_unverified(&identity, &state.verified_client_ids)
                        .then_some("unverified_client_at_approval");
                    let _ = crate::oauth_policy::record_audit(
                        &state,
                        Some(tenant.id),
                        "consent",
                        Some(identity.client_id.as_str()),
                        Some(identity.method),
                        None,
                        consent_reason,
                        Some(&ip),
                    )
                    .await;
                    // PRD-mcphost-oauth-first-grant-signup requirement 5: the
                    // tenant exists because of this grant -- one
                    // `first_grant_signup` event naming the client, in the
                    // tenant's own `host.oauth.audit` and the operator's
                    // `admin_audit` (`admin.audit_log`). Best-effort, same
                    // posture as the consent row above.
                    if first_grant_signup {
                        let _ = crate::oauth_policy::record_audit(
                            &state,
                            Some(tenant.id),
                            "first_grant_signup",
                            Some(identity.client_id.as_str()),
                            Some(identity.method),
                            None,
                            None,
                            Some(&ip),
                        )
                        .await;
                        let _ = state
                            .db
                            .record_admin_audit(
                                "oauth".to_string(),
                                "first_grant_signup".to_string(),
                                Some(tenant.namespace.clone()),
                                Some(identity.client_id.clone()),
                            )
                            .await;
                    }
                    mint_code_and_redirect(
                        &state,
                        &tenant,
                        &identity,
                        &redirect_uri,
                        &code_challenge,
                        &resource,
                        &resolved_scope,
                        &state_param,
                        funnel_origin,
                    )
                    .await
                }
                Ok(crate::oauth_policy::ClientDecision::Pending) => {
                    let _ = crate::oauth_policy::record_audit(
                        &state,
                        Some(tenant.id),
                        "authorize_refused",
                        Some(identity.client_id.as_str()),
                        Some(identity.method),
                        None,
                        Some("pending_approval"),
                        Some(&ip),
                    )
                    .await;
                    let client_name = identity.client_name.clone().unwrap_or_else(|| identity.client_id.clone());
                    html_response(StatusCode::OK, render_pending_page(&client_name))
                }
                Ok(crate::oauth_policy::ClientDecision::Refused(reason)) => {
                    let _ = crate::oauth_policy::record_audit(
                        &state,
                        Some(tenant.id),
                        "authorize_refused",
                        Some(identity.client_id.as_str()),
                        Some(identity.method),
                        None,
                        Some(reason),
                        Some(&ip),
                    )
                    .await;
                    render_inline_error(reason, "this client is not allowed to authorize for this tenant")
                }
                Err(_) => render_inline_error("server_error", "could not evaluate this tenant's client policy"),
            }
        }
        // requirement 6 / AC6: a tampered consent POST lacking a valid key
        // or claim code (or, requirement 2/AC7, one that names some OTHER
        // tenant than this per-tenant resource's own) re-renders the
        // consent page (never a redirect, and never a minted code) -- the
        // caller gets another chance rather than a hard failure, same
        // posture `claim::post_claim`'s own invalid-email branch takes.
        None => {
            let scope_groups = scope_groups_html(&state, resource_tenant.as_ref(), &resolved_scope).await;
            html_response(
                StatusCode::OK,
                render_consent_page(
                    &identity,
                    &state.verified_client_ids,
                    &resource,
                    &params,
                    &resolved_scope,
                    &scope_groups,
                    Some("that key or claim code did not verify -- try again"),
                    resource_tenant.is_none(),
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

pub(crate) fn oauth_error_json(status: StatusCode, error: &str, description: &str) -> Response {
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
    /// PRD-mcphost-enterprise-managed-auth requirement 3: the identity
    /// assertion JWT, for `grant_type=urn:ietf:params:oauth:grant-type:
    /// jwt-bearer`.
    #[serde(default)]
    pub assertion: Option<String>,
    /// requirement 3/4 (AC4): the requested scope for the JWT-bearer
    /// grant -- must be a subset of `mcp`/`catalog`, else `invalid_scope`.
    #[serde(default)]
    pub scope: Option<String>,
}

/// RFC 7636 S256: `BASE64URL-ENCODE(SHA256(code_verifier)) == code_challenge`.
fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize()) == challenge
}

/// requirement 4/PRD-mcphost-enterprise-managed-auth requirement 3: mints
/// and stores a fresh access token (and, when `mint_refresh`, a refresh
/// token too) for an already-created-or-rotated grant -- the one success
/// path `authorization_code`/`refresh_token` (via the [`issue_tokens`]
/// wrapper below, always `mint_refresh: true`) and the JWT-bearer grant
/// (`crate::assertion::token_jwt_bearer`, a namespaced `subject`,
/// `extra_claims` carrying `mcphost_tenant`/`email`/`name`, and
/// `mint_refresh` gated on the assertion's own refresh permission, AC6) all
/// converge on. `audit_event` is `"token"`/`"refresh"`/`"xaa"` (requirement
/// 4) -- the only thing that differs between the authorization_code/
/// refresh_token/JWT-bearer call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn issue_tokens_for_subject(
    state: &AppState,
    grant_id: i64,
    tenant_id: i64,
    resource: &str,
    scope: &str,
    client_id: &str,
    subject: &str,
    extra_claims: Map<String, Value>,
    mint_refresh: bool,
    audit_event: &str,
) -> Response {
    // PRD-mcphost-oauth-client-policy requirement 1: a tenant's own
    // `access_ttl_s`/`refresh_ttl_s`, falling back to this PRD's defaults
    // (equal to the hosted AS PRD's own fixed constants) for a tenant with
    // no policy row (AC9).
    let policy = crate::oauth_policy::OauthPolicy::load(state, tenant_id).await.unwrap_or_default();
    let now = crate::state::now_unix();
    let jti = crate::state::new_ulid();
    let exp = now + policy.access_ttl_s;
    // PRD-mcphost-tool-scopes-and-consent requirement 2: `scope` is the
    // real granted scope string (not a fixed "mcp"), threaded through by
    // every caller -- `issue_tokens`'s own authorization_code/refresh_token
    // callers pass the grant's own recorded scope; the JWT-bearer (`xaa`)
    // caller (PRD-mcphost-enterprise-managed-auth requirement 4) passes its
    // own resolved scope the same way.
    let mut claims_map = Map::new();
    claims_map.insert("iss".to_string(), json!(state.public_url.trim_end_matches('/')));
    claims_map.insert("sub".to_string(), json!(subject));
    claims_map.insert("aud".to_string(), json!(resource));
    claims_map.insert("scope".to_string(), json!(scope));
    claims_map.insert("client_id".to_string(), json!(client_id));
    claims_map.insert("jti".to_string(), json!(jti));
    claims_map.insert("iat".to_string(), json!(now));
    claims_map.insert("exp".to_string(), json!(exp));
    for (k, v) in extra_claims {
        claims_map.insert(k, v);
    }
    let claims = Value::Object(claims_map);
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
    let mut body = json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": policy.access_ttl_s,
        "scope": scope,
    });
    if mint_refresh {
        let refresh_token = crate::auth::generate_key();
        let refresh_hash = crate::auth::hash_key(&refresh_token);
        if state
            .db
            .insert_oauth_refresh_token(grant_id, refresh_hash, resource.to_string(), now + policy.refresh_ttl_s)
            .await
            .is_err()
        {
            return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not record refresh token");
        }
        if let Some(obj) = body.as_object_mut() {
            obj.insert("refresh_token".to_string(), json!(refresh_token));
        }
    }
    let _ = state.db.touch_oauth_grant_last_used(grant_id, now).await;
    let _ = crate::oauth_policy::record_audit(
        state,
        Some(tenant_id),
        audit_event,
        Some(client_id),
        None,
        None,
        None,
        None,
    )
    .await;
    // PRD-mcphost-oauth-demand-signal requirement 3 (AC4): every successful
    // exchange through this shared success path -- both the initial
    // `authorization_code` grant and a later `refresh_token` rotation --
    // counts toward `tokens_issued_7d`. Best-effort: never fails the
    // response that carries the real tokens.
    //
    // PRD-mcphost-funnel-truth AC1: `"human"` is a documented
    // simplification, not a live classification -- this shared success
    // path (`POST /oauth/token`, every grant type) carries no caller IP
    // (unlike `get_authorize`/`post_authorize` above, it was never given a
    // `ConnectInfo`/`HeaderMap` extractor, and widening it would touch
    // every grant-type handler for a count no digest/host_progress AC
    // reads). Defaulting to `human` (never `unknown`, satisfying AC1)
    // is conservative in the direction that matters: it can only
    // undercount fleet/probe token exchanges, never inflate the human
    // headline by hiding a real fleet/probe row inside it on a table
    // `admin::funnel`/`funnel_7d_external` don't aggregate from anyway.
    let _ = state.db.record_oauth_funnel_event("token_issued", "human").await;
    (StatusCode::OK, Json(body)).into_response()
}

/// requirement 4: mints and stores a fresh access/refresh token pair for
/// an already-created-or-rotated grant -- the one success path
/// `authorization_code`/`refresh_token`/JWT-bearer grants all converge on.
///
/// PRD-mcphost-federated-end-user-login requirement 3: when `grant_id`
/// carries a federated end user (looked up here rather than threaded
/// through both call sites, since the refresh-token path already has the
/// grant row in scope and the authorization-code path doesn't need a
/// second one), `sub` is that user's own namespaced subject and
/// `mcphost_tenant`/`email`/`name`/`end_user_issuer` ride along; otherwise
/// `sub` stays the tenant's own `namespace`, unchanged from before this PRD.
#[allow(clippy::too_many_arguments)]
async fn issue_tokens(state: &AppState, grant_id: i64, tenant_id: i64, resource: &str, scope: &str, client_id: &str, audit_event: &str) -> Response {
    let tenant = match state.db.find_tenant_by_id(tenant_id).await {
        Ok(Some(t)) => t,
        _ => return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "tenant not found"),
    };
    let end_user = match state.db.find_oauth_grant_by_id(grant_id).await {
        Ok(Some(g)) => g.end_user(),
        _ => None,
    };
    let subject = end_user.as_ref().and_then(|e| e.subject.clone()).unwrap_or_else(|| tenant.namespace.clone());
    let mut extra_claims = Map::new();
    if let Some(eu) = &end_user {
        extra_claims.insert("mcphost_tenant".to_string(), json!(tenant.namespace));
        if let Some(email) = &eu.email {
            extra_claims.insert("email".to_string(), json!(email));
        }
        if let Some(name) = &eu.name {
            extra_claims.insert("name".to_string(), json!(name));
        }
        if let Some(issuer) = &eu.issuer {
            extra_claims.insert("end_user_issuer".to_string(), json!(issuer));
        }
    }
    issue_tokens_for_subject(state, grant_id, tenant_id, resource, scope, client_id, &subject, extra_claims, true, audit_event).await
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
    // PRD-mcphost-oauth-client-policy requirement 5 (AC7): "exchanges" is
    // one of the three actions a blocked client is refused at.
    if crate::oauth_policy::is_client_blocked(state, &identity).await.unwrap_or(false) {
        let _ = crate::oauth_policy::record_audit(
            state,
            Some(row.tenant_id),
            "token_refused",
            Some(client_id),
            Some(identity.method),
            None,
            Some("client_blocked"),
            None,
        )
        .await;
        return oauth_error_json(StatusCode::FORBIDDEN, "invalid_client", "client_blocked");
    }
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
        .create_oauth_grant(
            row.tenant_id,
            row.client_id.clone(),
            row.client_name.clone(),
            row.method.clone(),
            row.resource.clone(),
            row.scope.clone(),
            row.end_user(),
        )
        .await
    {
        Ok(id) => id,
        Err(_) => return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not create grant"),
    };
    let _ = state.db.set_oauth_code_grant(code_hash, grant_id).await;
    issue_tokens(state, grant_id, row.tenant_id, &row.resource, &row.scope, client_id, "token").await
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
    let Ok(Some(grant)) = state.db.find_oauth_grant_by_id(row.grant_id).await else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "grant not found");
    };
    if grant.revoked_unix.is_some() {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "grant has been revoked");
    }
    // PRD-mcphost-oauth-client-policy requirement 5 (AC7): "exchanges"
    // covers a refresh too.
    if crate::oauth_policy::is_client_id_or_method_blocked(state, &grant.client_id, &grant.method)
        .await
        .unwrap_or(false)
    {
        let _ = crate::oauth_policy::record_audit(
            state,
            Some(grant.tenant_id),
            "refresh_refused",
            Some(grant.client_id.as_str()),
            Some(grant.method.as_str()),
            None,
            Some("client_blocked"),
            None,
        )
        .await;
        return oauth_error_json(StatusCode::FORBIDDEN, "invalid_client", "client_blocked");
    }
    // requirement 1/2 (AC3/AC4): `max_grant_age_s` is this grant's hard
    // outer bound (checked first: once past it, `grant_expired` always
    // wins over `reconsent_required` even if both thresholds are already
    // crossed); `reconsent_after_s` forces the consent page again well
    // before that, on a still-otherwise-live grant.
    let policy = crate::oauth_policy::OauthPolicy::load(state, grant.tenant_id).await.unwrap_or_default();
    let grant_age = now - grant.created_unix;
    if let Some(max_age) = policy.max_grant_age_s
        && grant_age > max_age
    {
        let _ = crate::oauth_policy::record_audit(
            state,
            Some(grant.tenant_id),
            "refresh_refused",
            Some(grant.client_id.as_str()),
            Some(grant.method.as_str()),
            None,
            Some("grant_expired"),
            None,
        )
        .await;
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "grant_expired");
    }
    if let Some(reconsent_after) = policy.reconsent_after_s
        && grant_age > reconsent_after
    {
        let _ = crate::oauth_policy::record_audit(
            state,
            Some(grant.tenant_id),
            "refresh_refused",
            Some(grant.client_id.as_str()),
            Some(grant.method.as_str()),
            None,
            Some("reconsent_required"),
            None,
        )
        .await;
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "reconsent_required");
    }
    // This specific refresh token's own TTL (`policy.refresh_ttl_s` at the
    // time it was minted) -- checked last among the "why is this refusal
    // an expiry" reasons, so a grant already past `max_grant_age_s`/
    // `reconsent_after_s` always reports that instead, even when this
    // token's own TTL happens to be shorter and would otherwise fire
    // first.
    if now > row.expires_unix {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "refresh token expired");
    }
    if !state.db.claim_oauth_refresh_token(hash, now).await.unwrap_or(false) {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", "refresh token already rotated");
    }
    issue_tokens(state, row.grant_id, grant.tenant_id, &row.resource, &grant.scope, &grant.client_id, "refresh").await
}

/// `POST /oauth/token` (requirement 4; AC4, AC5, AC7).
pub async fn post_token(State(state): State<Arc<AppState>>, axum::Form(req): axum::Form<TokenRequest>) -> Response {
    match req.grant_type.as_deref() {
        Some("authorization_code") => token_authorization_code(&state, &req).await,
        Some("refresh_token") => token_refresh_token(&state, &req).await,
        // PRD-mcphost-enterprise-managed-auth requirement 3: the RFC 7523
        // JWT-bearer grant.
        Some(g) if g == crate::assertion::JWT_BEARER_GRANT_TYPE => {
            crate::assertion::token_jwt_bearer(&state, &req).await
        }
        _ => oauth_error_json(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "grant_type must be \"authorization_code\", \"refresh_token\", or the JWT-bearer URN",
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
/// path. The tenant and the token's resource are both resolved from the
/// `jti`'s own `oauth_jti_denylist` row (never from `sub`/`aud` directly):
/// PRD-mcphost-enterprise-managed-auth's JWT-bearer grant mints `sub` as
/// `<issuer>#<subject>` (namespaced, requirement 3) rather than a tenant
/// namespace, and PRD-mcphost-federated-end-user-login's federated grant
/// mints `sub` as the end user's own namespaced subject; both PRDs' `aud`
/// is a per-tenant resource (`crate::authz::per_tenant_resource`/
/// `tenant_resource`) rather than the fixed root one -- all already
/// recorded on the jti row at mint time (`issue_tokens_for_subject`), so
/// re-deriving tenant/resource from the row is correct for every grant
/// this host has ever minted, old or new. A `jti` absent from
/// `oauth_jti_denylist` (this crate never mints one without recording it)
/// or found `denied_unix` both refuse as `"revoked"`. `oauth_grants.method
/// == "xaa"` is what tells an enterprise-assertion-derived token apart
/// from every other hosted token (requirement 5): its `auth_method`
/// becomes `"enterprise_assertion"` rather than `"hosted_token"`; a grant
/// with a federated end user attached (PRD-mcphost-federated-end-user-login
/// requirement 3) instead becomes `"federated"`. Either way, the `email`/
/// `name` claims (present only on those grants' tokens) are copied onto
/// the returned [`crate::oauth::OauthCaller`].
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
    // PRD-mcphost-enterprise-managed-auth requirement 5,
    // PRD-mcphost-federated-end-user-login requirement 2, and
    // PRD-mcphost-tool-scopes-and-consent requirement 2: all three PRDs'
    // grants mint `aud` as a per-tenant resource rather than the fixed root
    // one, so the check is against the jti row's own recorded resource (set
    // at mint time by `issue_tokens_for_subject`) rather than recomputing a
    // per-tenant resource from a tenant looked up off the claims -- the
    // tenant/issuer/auth_method every PRD needs (including this one's own
    // `enterprise_assertion` method) are derived once, below, from
    // `jti_row`/`grant` instead.
    if aud != jti_row.resource {
        return Err(AppError::InvalidToken("wrong_audience"));
    }
    let tenant = state
        .db
        .find_tenant_by_id(jti_row.tenant_id)
        .await?
        .ok_or(AppError::InvalidToken("malformed"))?;
    let grant = state.db.find_oauth_grant_by_id(jti_row.grant_id).await?;
    let is_assertion_grant = grant.as_ref().is_some_and(|g| g.method == "xaa");
    // PRD-mcphost-federated-end-user-login requirement 3: a federated
    // token's `sub` is the end user's own namespaced subject, not the
    // tenant's `namespace`, and `issuer` is the provider's own issuer
    // (`end_user_issuer`), never this host's own `public_url`.
    let end_user = grant.as_ref().and_then(|g| g.end_user());
    let (issuer, auth_method) = if is_assertion_grant {
        (state.public_url.trim_end_matches('/').to_string(), "enterprise_assertion")
    } else if end_user.is_some() {
        let issuer = claims
            .get("end_user_issuer")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(AppError::InvalidToken("malformed"))?;
        (issuer, "federated")
    } else {
        (state.public_url.trim_end_matches('/').to_string(), "hosted_token")
    };
    let is_federated = end_user.is_some();
    let _ = state.db.touch_oauth_grant_last_used(jti_row.grant_id, now).await;

    let scope = claims.get("scope").and_then(Value::as_str).map(str::to_string);
    Ok(crate::oauth::OauthCaller {
        tenant_id: tenant.id,
        subject: sub,
        issuer,
        auth_method,
        scope,
        email: (is_assertion_grant || is_federated)
            .then(|| claims.get("email").and_then(Value::as_str).map(str::to_string))
            .flatten(),
        name: (is_assertion_grant || is_federated)
            .then(|| claims.get("name").and_then(Value::as_str).map(str::to_string))
            .flatten(),
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
                // PRD-mcphost-tool-scopes-and-consent requirement 6: the
                // granted scopes this grant was actually issued with.
                "scope": r.scope,
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
