//! Test-only fake OIDC provider fixtures for the `fedlogin_ac*.rs` suite
//! (PRD-mcphost-federated-end-user-login): a `wiremock` server serving
//! discovery, JWKS, and token-exchange endpoints, plus an `id_token`
//! minter. The provider's own `authorize` endpoint is never actually hit
//! in these tests -- `GET /oauth/authorize`'s 302 `Location` is asserted
//! directly, and "the provider redirecting back" is simulated by calling
//! `GET /oauth/federation/callback` with a fabricated `code`, the same
//! simplification `vault_ac02`'s own upstream-provider fixture makes.
#![allow(dead_code)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use std::sync::OnceLock;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const KID_1: &str = "fed-test-key-1";

/// An EC (P-256) test keypair -- see `tests/support/oauth.rs::GenKey` for
/// the identical shape and rationale (fresh per test process via `rcgen`,
/// not an embedded PEM literal).
struct GenKey {
    priv_pem: String,
    x: String,
    y: String,
}

fn gen_ec_key() -> GenKey {
    let kp = rcgen::KeyPair::generate().expect("generate EC P-256 test keypair");
    let priv_pem = kp.serialize_pem();
    let raw = kp.public_key_raw();
    assert_eq!(raw.len(), 65, "expected an uncompressed P-256 point");
    let x = URL_SAFE_NO_PAD.encode(&raw[1..33]);
    let y = URL_SAFE_NO_PAD.encode(&raw[33..65]);
    GenKey { priv_pem, x, y }
}

/// [`KID_1`]'s keypair -- the one [`jwk_1`]/the fake provider's `/jwks`
/// publishes.
fn key_1() -> &'static GenKey {
    static KEY: OnceLock<GenKey> = OnceLock::new();
    KEY.get_or_init(gen_ec_key)
}

/// A second, unrelated EC keypair whose public half is never published in
/// the fake provider's JWKS -- AC4's "id_token signed by another key"
/// signs with this key while the JWT header still names [`KID_1`].
fn key_2() -> &'static GenKey {
    static KEY: OnceLock<GenKey> = OnceLock::new();
    KEY.get_or_init(gen_ec_key)
}

pub fn priv_pem_1() -> &'static str {
    &key_1().priv_pem
}

pub fn priv_pem_2() -> &'static str {
    &key_2().priv_pem
}

pub fn jwk_1() -> Value {
    let k = key_1();
    json!({"kty": "EC", "kid": KID_1, "crv": "P-256", "x": k.x, "y": k.y})
}

/// A running fake OIDC provider: discovery and JWKS are mounted
/// immediately; `/token` is mounted per test via [`mount_token`] once the
/// test knows what `id_token` it wants returned.
pub struct FakeProvider {
    pub server: MockServer,
}

impl FakeProvider {
    pub fn issuer(&self) -> String {
        self.server.uri()
    }

    pub fn jwks_uri(&self) -> String {
        format!("{}/jwks", self.server.uri())
    }

    pub fn token_endpoint(&self) -> String {
        format!("{}/token", self.server.uri())
    }

    pub fn authorization_endpoint(&self) -> String {
        format!("{}/authorize", self.server.uri())
    }
}

/// Starts a fake provider with discovery (AC1) and JWKS ([`jwk_1`]) wired.
pub async fn start() -> FakeProvider {
    let server = MockServer::start().await;
    let issuer = server.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "jwks_uri": format!("{issuer}/jwks"),
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"keys": [jwk_1()]})))
        .mount(&server)
        .await;
    FakeProvider { server }
}

/// A discovery document down (AC8's "discovery endpoint down"): JWKS still
/// answers, `.well-known/openid-configuration` 500s.
pub async fn start_with_discovery_down() -> FakeProvider {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"keys": [jwk_1()]})))
        .mount(&server)
        .await;
    FakeProvider { server }
}

/// AC8: takes an already-`start()`-ed provider's discovery endpoint down
/// (jwks stays up) -- used after `host.oauth.provider_set` already
/// succeeded once against the live discovery document, to prove
/// `host.oauth.doctor`'s own discovery check is a fresh live re-fetch, not
/// a replay of what `provider_set` cached.
pub async fn take_discovery_down(provider: &FakeProvider) {
    provider.server.reset().await;
    Mock::given(method("GET"))
        .and(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&provider.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"keys": [jwk_1()]})))
        .mount(&provider.server)
        .await;
}

/// Mounts `POST /token` to always return `id_token` (plus a fixed dummy
/// `access_token`) regardless of the `code` presented -- these tests key
/// identity off the `id_token`'s own claims, never the upstream `code`
/// itself, so one canned response per test is enough.
pub async fn mount_token(provider: &FakeProvider, id_token: &str) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fake-upstream-access-token",
            "token_type": "Bearer",
            "expires_in": 3600,
            "id_token": id_token,
        })))
        .mount(&provider.server)
        .await;
}

/// Mounts `POST /token` to return a provider-side error (AC4's
/// `token_exchange_failed`-adjacent path is exercised elsewhere via the
/// `error=` callback query param instead; this is for completeness /
/// future use).
pub async fn mount_token_failing(provider: &FakeProvider) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})))
        .mount(&provider.server)
        .await;
}

/// An `id_token` claim set builder -- every field a test might want to
/// tamper with (`nonce`, `aud`, `exp`, `email_verified`, the signing key
/// itself) is an explicit argument rather than a fixed shape.
#[allow(clippy::too_many_arguments)]
pub fn sign_id_token(
    kid: &str,
    priv_pem: &str,
    issuer: &str,
    client_id: &str,
    sub: &str,
    nonce: &str,
    email: Option<&str>,
    email_verified: Option<bool>,
    name: Option<&str>,
    exp_offset_secs: i64,
) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut claims = json!({
        "iss": issuer,
        "aud": client_id,
        "sub": sub,
        "nonce": nonce,
        "iat": now,
        "exp": now + exp_offset_secs,
    });
    if let Some(obj) = claims.as_object_mut() {
        if let Some(e) = email {
            obj.insert("email".to_string(), json!(e));
        }
        if let Some(v) = email_verified {
            obj.insert("email_verified".to_string(), json!(v));
        }
        if let Some(n) = name {
            obj.insert("name".to_string(), json!(n));
        }
    }
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    let key = EncodingKey::from_ec_pem(priv_pem.as_bytes()).expect("valid EC PEM");
    encode(&header, &claims, &key).expect("sign test id_token")
}

/// Parses a `key=value&...` query string -- same tiny helper every
/// `hostedas_ac*` test already hand-rolls for reading a redirect's query.
pub fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}
