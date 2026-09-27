//! PRD-mcphost-enterprise-managed-auth: RFC 7523 JWT-bearer grant
//! (`grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`) at `POST
//! /oauth/token` -- an admin-trusted identity provider mints an identity
//! assertion for one of its employees; Claude presents it here with no
//! interactive consent, and this module validates it and mints a token via
//! `authz.rs`'s existing grant/jti/refresh-token machinery. `oauth.rs` owns
//! the per-tenant trusted-issuer registry (`host.oauth.trusted_issuer_*`)
//! and the shared JWKS cache this module's signature check reuses
//! (technical considerations: "cache shared with issuer JWTs").

use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::authz::{TokenRequest, oauth_error_json};
use crate::oauth::SignatureResolution;
use crate::state::AppState;

/// RFC 7523 section 2.1's own grant-type URN.
pub const JWT_BEARER_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// requirement 3: "`exp`/`iat`/`nbf` (max 5 min skew)" -- distinct from
/// `oauth.rs`/`authz.rs`'s own 60s skew for issuer JWTs/hosted tokens; an
/// identity assertion is a short-lived, freshly-minted credential (AC1:
/// "`exp` in 5 min"), not a long-lived issuer-signed bearer.
const ASSERTION_CLOCK_SKEW_SECS: i64 = 300;

/// requirement 3: `scope` (when the token request names one at all) must
/// be a subset of this set -- `mcp` (the only scope every other grant in
/// this crate already mints) and `catalog` (read-only catalog access,
/// named in requirement 3's "`mcp` union catalog").
const ALLOWED_SCOPES: &[&str] = &["mcp", "catalog"];

/// requirement 3 (draft-ietf-oauth-identity-assertion-authz-grant-04 does
/// not actually define a refresh-permission claim name -- see this PRD's
/// technical considerations and the intent card's `ambiguities_resolved`):
/// the boolean claim mcphost's own identity assertions carry to opt into a
/// refresh token, named after OIDC's own `offline_access` scope concept
/// (AC6).
const OFFLINE_ACCESS_CLAIM: &str = "offline_access";

fn invalid_grant(description: &str) -> Response {
    oauth_error_json(StatusCode::BAD_REQUEST, "invalid_grant", description)
}

/// requirement 4: every assertion rejection that has identified a trusted
/// issuer names it and the reason in `oauth_rejections` (shared with
/// `oauth.rs`'s own per-issuer counters) -- never the assertion itself.
async fn audit_rejection(state: &AppState, issuer: &str, reason: &str) {
    let _ = state.db.increment_oauth_rejection(issuer.to_string(), reason.to_string()).await;
}

/// `POST /oauth/token` with `grant_type=urn:ietf:params:oauth:grant-type:
/// jwt-bearer` (requirement 3; AC1-AC8).
pub async fn token_jwt_bearer(state: &AppState, req: &TokenRequest) -> Response {
    let Some(assertion) = req.assertion.as_deref().filter(|s| !s.is_empty()) else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_request", "assertion is required");
    };
    let Some(client_id) = req.client_id.as_deref().filter(|s| !s.is_empty()) else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_request", "client_id is required");
    };
    let Some(resource) = req.resource.as_deref().filter(|s| !s.is_empty()) else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_target", "resource is required");
    };

    // requirement 3/AC1/AC4: `resource` names the tenant this grant is for
    // (`<public>/t/<ns>/mcp`) -- an unparseable shape or an unknown `<ns>`
    // is `invalid_target`, never leaking whether `<ns>` almost existed.
    let Some(namespace) = crate::authz::resource_tenant_namespace(state, resource) else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_target", "resource is not a recognized tenant resource URI");
    };
    let Ok(Some(tenant)) = state.db.find_tenant_by_namespace(namespace.to_string()).await else {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_target", "resource names no known tenant");
    };

    // requirement 3: peek `iss` (unverified -- every claim this function
    // actually trusts is re-read from the signature-verified payload
    // below) to find which trusted issuer (if any) this assertion claims
    // to be from.
    let Some(peek) = crate::oauth::peek_claims(assertion) else {
        return invalid_grant("assertion is malformed");
    };
    let Some(iss) = peek.get("iss").and_then(Value::as_str).map(str::to_string) else {
        return invalid_grant("assertion is missing iss");
    };

    let Some(issuer_row) = state.db.find_trusted_issuer_by_issuer(iss.clone()).await.unwrap_or(None) else {
        audit_rejection(state, &iss, "untrusted_issuer").await;
        return invalid_grant("issuer is not trusted for any tenant");
    };
    if issuer_row.tenant_id != tenant.id {
        audit_rejection(state, &iss, "untrusted_issuer").await;
        return invalid_grant("issuer is not trusted for this resource's tenant");
    }

    // requirement 1/AC4: only the pre-registered enterprise client may use
    // the grant for this issuer.
    if client_id != issuer_row.client_id {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_client", "client_id is not registered for this issuer");
    }

    // requirement 3/AC4: `scope` (when present) must be mcp/catalog only.
    let scope = req.scope.as_deref().filter(|s| !s.is_empty()).unwrap_or("mcp");
    if !scope.split_whitespace().all(|s| ALLOWED_SCOPES.contains(&s)) {
        return oauth_error_json(StatusCode::BAD_REQUEST, "invalid_scope", "scope must be a subset of mcp, catalog");
    }

    let claims = match crate::oauth::verify_signed_jwt(state, &issuer_row.issuer, &issuer_row.jwks_url, assertion).await {
        SignatureResolution::Verified(claims) => claims,
        SignatureResolution::UnknownIssuer => {
            audit_rejection(state, &iss, "jwks_unavailable").await;
            return invalid_grant("could not fetch this issuer's JWKS");
        }
        SignatureResolution::BadSignature => {
            audit_rejection(state, &iss, "bad_signature").await;
            return invalid_grant("assertion signature is invalid");
        }
    };

    let Some(sub) = claims.get("sub").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        audit_rejection(state, &iss, "malformed").await;
        return invalid_grant("assertion is missing sub");
    };
    let Some(aud) = claims.get("aud").and_then(Value::as_str) else {
        audit_rejection(state, &iss, "malformed").await;
        return invalid_grant("assertion is missing aud");
    };
    let expected_aud = issuer_row
        .audience
        .clone()
        .unwrap_or_else(|| state.public_url.trim_end_matches('/').to_string());
    if aud != expected_aud {
        audit_rejection(state, &iss, "wrong_audience").await;
        return invalid_grant("assertion aud does not match this authorization server");
    }
    let Some(exp) = claims.get("exp").and_then(Value::as_i64) else {
        audit_rejection(state, &iss, "malformed").await;
        return invalid_grant("assertion is missing exp");
    };
    let now = crate::state::now_unix();
    if now > exp + ASSERTION_CLOCK_SKEW_SECS {
        audit_rejection(state, &iss, "expired").await;
        return invalid_grant("assertion has expired");
    }
    if let Some(nbf) = claims.get("nbf").and_then(Value::as_i64)
        && now + ASSERTION_CLOCK_SKEW_SECS < nbf
    {
        audit_rejection(state, &iss, "expired").await;
        return invalid_grant("assertion is not yet valid");
    }
    if let Some(iat) = claims.get("iat").and_then(Value::as_i64)
        && iat > now + ASSERTION_CLOCK_SKEW_SECS
    {
        audit_rejection(state, &iss, "expired").await;
        return invalid_grant("assertion iat is too far in the future");
    }
    let Some(jti) = claims.get("jti").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        audit_rejection(state, &iss, "malformed").await;
        return invalid_grant("assertion is missing jti");
    };

    // requirement 3/4 (AC3): the replay guard -- the insert itself is the
    // atomic claim, so two concurrent presentations of the same assertion
    // can only ever have one of them win.
    match state.db.claim_assertion_jti(iss.clone(), jti.to_string(), exp).await {
        Ok(true) => {}
        Ok(false) => {
            audit_rejection(state, &iss, "replay").await;
            return invalid_grant("assertion jti has already been used");
        }
        Err(_) => return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "storage error"),
    }

    // requirement 5/AC7: `sub` is namespaced by the issuer (requirement 3:
    // "sub namespaced by the issuer") -- the same subject
    // `host.enduser.revoke`/`unrevoke` and the per-call revoked check in
    // `handler.rs` key on.
    let subject = format!("{iss}#{sub}");
    match state.db.is_end_user_revoked(tenant.id, subject.clone()).await {
        Ok(true) => {
            audit_rejection(state, &iss, "subject_revoked").await;
            return invalid_grant("subject_revoked");
        }
        Ok(false) => {}
        Err(_) => return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "storage error"),
    }

    let email = claims.get("email").and_then(Value::as_str).map(str::to_string);
    let name = claims.get("name").and_then(Value::as_str).map(str::to_string);
    let mint_refresh = claims.get(OFFLINE_ACCESS_CLAIM).and_then(Value::as_bool).unwrap_or(false);

    let grant_id = match state
        .db
        .create_oauth_grant(tenant.id, client_id.to_string(), None, "xaa".to_string(), resource.to_string(), scope.to_string(), None)
        .await
    {
        Ok(id) => id,
        Err(_) => return oauth_error_json(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "could not create grant"),
    };

    let mut extra_claims = Map::new();
    extra_claims.insert("mcphost_tenant".to_string(), json!(tenant.namespace));
    if let Some(email) = &email {
        extra_claims.insert("email".to_string(), json!(email));
    }
    if let Some(name) = &name {
        extra_claims.insert("name".to_string(), json!(name));
    }

    crate::authz::issue_tokens_for_subject(state, grant_id, tenant.id, resource, scope, client_id, &subject, extra_claims, mint_refresh, "xaa")
        .await
}
