//! Test-only OAuth fixtures for the `oauthrs_ac*.rs` suite
//! (PRD-mcphost-oauth-resource-server): an ES256 test keypair (and a
//! second, unrelated one for the wrong-signature case) generated fresh
//! each test process, a JWT minter, and a `wiremock` JWKS server.
#![allow(dead_code)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use std::sync::OnceLock;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The `kid` [`jwk_1`] publishes and [`sign`] defaults to.
pub const KID_1: &str = "test-key-1";

/// An EC (P-256) test keypair: the PKCS8 PEM `sign` signs with, plus the
/// raw public-key point's `x`/`y` coordinates for the matching JWK.
/// Generated once per test process (not embedded PEM literals, which a
/// secrets scanner correctly flags even for test-only keys) via `rcgen`.
struct GenKey {
    priv_pem: String,
    x: String,
    y: String,
}

fn gen_ec_key() -> GenKey {
    let kp = rcgen::KeyPair::generate().expect("generate EC P-256 test keypair");
    let priv_pem = kp.serialize_pem();
    // rcgen's `public_key_raw` for an EC key is the uncompressed SEC1
    // point: 0x04 || X (32 bytes) || Y (32 bytes).
    let raw = kp.public_key_raw();
    assert_eq!(raw.len(), 65, "expected an uncompressed P-256 point");
    let x = URL_SAFE_NO_PAD.encode(&raw[1..33]);
    let y = URL_SAFE_NO_PAD.encode(&raw[33..65]);
    GenKey { priv_pem, x, y }
}

/// [`KID_1`]'s keypair -- the one [`jwk_1`] publishes.
fn key_1() -> &'static GenKey {
    static KEY: OnceLock<GenKey> = OnceLock::new();
    KEY.get_or_init(gen_ec_key)
}

/// A second, unrelated EC keypair whose private half never appears in any
/// published JWKS -- AC3's wrong-signature case signs with this key while
/// the JWT header still names [`KID_1`], so signature verification against
/// the real (published) key fails.
fn key_2() -> &'static GenKey {
    static KEY: OnceLock<GenKey> = OnceLock::new();
    KEY.get_or_init(gen_ec_key)
}

/// PKCS8 EC (P-256) private key matching [`jwk_1`]'s published key.
pub fn priv_pem_1() -> &'static str {
    &key_1().priv_pem
}

/// PKCS8 EC (P-256) private key for [`key_2`] -- see its doc comment.
pub fn priv_pem_2() -> &'static str {
    &key_2().priv_pem
}

/// The JWK [`jwks_server`] publishes by default: [`KID_1`]'s public half.
pub fn jwk_1() -> Value {
    let k = key_1();
    json!({"kty": "EC", "kid": KID_1, "crv": "P-256", "x": k.x, "y": k.y})
}

/// A signed JWT: `iss`/`aud`/`sub` as given, `exp` (and `iat`) computed
/// from `now_unix + exp_offset_secs` (negative for an already-expired
/// token), signed with `priv_pem` under header `kid`. `priv_pem` need not
/// match `kid`'s published key (AC3's wrong-signature case relies on
/// exactly that mismatch).
#[allow(clippy::too_many_arguments)]
pub fn sign(kid: &str, priv_pem: &str, iss: &str, aud: &str, sub: &str, exp_offset_secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = json!({
        "iss": iss,
        "aud": aud,
        "sub": sub,
        "iat": now,
        "exp": now + exp_offset_secs,
    });
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.to_string());
    let key = EncodingKey::from_ec_pem(priv_pem.as_bytes()).expect("valid EC PEM");
    encode(&header, &claims, &key).expect("sign test JWT")
}

/// A `wiremock` server whose `GET /jwks` always answers `{"keys": [jwk]}`.
/// `host.oauth.issuer_set`'s own `jwks_url` argument is
/// `format!("{}/jwks", server.uri())`.
pub async fn jwks_server(jwk: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/jwks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"keys": [jwk]})))
        .mount(&server)
        .await;
    server
}

/// A `jwks_url` nothing is listening on -- AC6's "the JWKS server is
/// down": port 1 is a privileged, unassigned port, so a connection to it
/// is refused immediately rather than hanging out to the 5s fetch timeout.
pub const DOWN_JWKS_URL: &str = "http://127.0.0.1:1/jwks";
