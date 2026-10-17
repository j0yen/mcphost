//! PRD-mcphost-openapi-for-rest-facade R1: the one table of public HTTP
//! routes. `build_router` registers every entry here by iterating
//! [`ROUTES`] (handler included, via [`RouteSpec::mount`]), so a route
//! cannot be served without a spec entry; [`crate::openapi::document`]
//! renders the same table. Routes registered any other way are exactly
//! [`UNSPECCED`], and `tests/oapi_ac02_*` walks the built router to prove it.

use std::sync::Arc;

use axum::routing::{MethodRouter, get, options, post};

use super::{
    billing_cancel, billing_done, billing_webhook, capabilities_json, connect_go, connect_page,
    contract_json_route, events_json_route, get_new_url, healthz, help_index, help_page,
    errors_json_route, incidents_json_route, plans_json_route, post_new_url, protocol_frames_json, status_json_route,
    well_known_authorization_server, well_known_authorization_server_tenant, well_known_authz_jwks,
    well_known_catalog, well_known_oauth_protected_resource, well_known_oauth_protected_resource_entry,
    well_known_oauth_protected_resource_tenant, well_known_server_json,
};
use crate::state::AppState;

/// Who may call a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    Anonymous,
    Admin,
    Bearer,
    /// The credential is a secret path segment (share token, signed id, claim code).
    Token,
    /// A provider-signed request (Stripe, inbound-mail provider).
    Signature,
}

/// A response or request body, by media type; `Json` names a schema in
/// [`crate::openapi::schemas`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaRef {
    Json(&'static str),
    Html,
    Binary,
    Empty,
}

pub type Mount = fn() -> MethodRouter<Arc<AppState>>;

#[derive(Clone, Copy)]
pub struct RouteSpec {
    /// Upper-case HTTP method.
    pub method: &'static str,
    /// axum path syntax; `{name}` segments are the OpenAPI path parameters.
    pub path: &'static str,
    pub tag: &'static str,
    pub summary: &'static str,
    /// Query parameter names (path parameters come from `path`).
    pub query: &'static [&'static str],
    pub request: Option<SchemaRef>,
    pub responses: &'static [(u16, SchemaRef)],
    pub auth: Auth,
    /// Wrap the path in `public_tool::add_cors_header` (the `/x/...` family).
    pub cors: bool,
    pub mount: Mount,
}

/// Routes registered outside [`ROUTES`] (R1): the MCP transports, tenant MCP
/// routers and the compile-time-embedded `www/*` pages. One allowlist, in
/// axum path syntax.
pub const UNSPECCED: &[&str] = &[
    "/mcp",
    "/t/{namespace}/mcp",
    "/u/{secret}/mcp",
    "/i/{code}/mcp",
    "/o/{code}/mcp",
    "/j/{code}/mcp",
    crate::oauth::OAUTH_ENTRY_PATH,
    "/status.html",
    "/aup.html",
    "/plans.html",
    "/llms.txt",
    "/llms-full.txt",
    "/skill.md",
    "/llms-install.md",
    "/sitemap.xml",
    "/support.html",
];

macro_rules! route {
    ($method:literal $path:literal $tag:literal $summary:literal $auth:ident
     query [$($q:literal),*] request $req:expr; cors $cors:literal;
     responses [$($st:literal => $body:expr),* $(,)?] => $mount:expr) => {
        RouteSpec {
            method: $method,
            path: $path,
            tag: $tag,
            summary: $summary,
            query: &[$($q),*],
            request: $req,
            responses: &[$(($st, $body)),*],
            auth: Auth::$auth,
            cors: $cors,
            mount: $mount,
        }
    };
    ($method:literal $path:literal $tag:literal $summary:literal $auth:ident
     responses [$($st:literal => $body:expr),* $(,)?] => $mount:expr) => {
        route!($method $path $tag $summary $auth query [] request None; cors false;
               responses [$($st => $body),*] => $mount)
    };
}

use SchemaRef::{Binary, Empty, Html, Json};

pub static ROUTES: &[RouteSpec] = &[
    // ---- public tool façade ------------------------------------------------
    route!("GET" "/x/{token}/{tool}" "public-tool" "Run a url-shared tool; the query string is the arguments" Token
        query [] request None; cors true;
        responses [200 => Json("RunResult"), 202 => Json("RunAccepted"), 400 => Json("ErrorEnvelope"),
                   404 => Empty, 429 => Json("ErrorEnvelope"), 500 => Json("ErrorEnvelope"), 503 => Json("ErrorEnvelope")]
        => || get(crate::public_tool::x_get)),
    route!("POST" "/x/{token}/{tool}" "public-tool" "Run a url-shared tool; the JSON body is the arguments" Token
        query [] request Some(Json("ToolArgs")); cors true;
        responses [200 => Json("RunResult"), 202 => Json("RunAccepted"), 400 => Json("ErrorEnvelope"),
                   404 => Empty, 429 => Json("ErrorEnvelope"), 500 => Json("ErrorEnvelope"), 503 => Json("ErrorEnvelope")]
        => || post(crate::public_tool::x_post)),
    route!("OPTIONS" "/x/{token}/{tool}" "public-tool" "CORS preflight" Anonymous
        query [] request None; cors true;
        responses [204 => Empty]
        => || options(crate::public_tool::x_options)),
    route!("GET" "/x/{token}/{tool}/runs/{run_id}" "public-tool" "Poll a run started through this token" Token
        query [] request None; cors true;
        responses [200 => Json("RunResult"), 202 => Json("RunAccepted"), 404 => Empty, 500 => Json("ErrorEnvelope"), 503 => Json("ErrorEnvelope")]
        => || get(crate::public_tool::x_poll)),
    route!("GET" "/f/{token}/{tool}" "public-tool" "Argument form for a url-shared tool with ui.form" Token
        responses [200 => Html, 404 => Empty]
        => || get(crate::form_page::form_get)),
    route!("POST" "/f/{token}/{tool}" "public-tool" "Submit the argument form" Token
        responses [200 => Html, 404 => Empty]
        => || post(crate::form_page::form_post)),

    route!("GET" "/d/{namespace}/{tool}" "public-tool" "Try-it demo page for a catalog tool" Anonymous
        responses [200 => Html, 404 => Empty]
        => || get(crate::demo_page::demo_get)),
    // ---- status / discovery -------------------------------------------------
    route!("GET" "/healthz" "status" "Liveness probe" Anonymous
        responses [200 => Json("Healthz"), 503 => Json("Healthz")]
        => || get(healthz)),
    route!("GET" "/reach" "status" "Content-free reachability probe for any public hostname" Anonymous
        responses [200 => Json("Reach")]
        => || get(crate::reach::reach)),
    route!("GET" "/status.json" "status" "Status feed; component and days select one component's daily rollup" Anonymous
        query ["component", "days"] request None; cors false;
        responses [200 => Json("StatusFeed"), 500 => Json("LegacyError")]
        => || get(status_json_route)),
    route!("GET" "/incidents.json" "status" "Last 24 h of heal actions, anonymised" Anonymous
        responses [200 => Json("Incidents"), 500 => Json("LegacyError")]
        => || get(incidents_json_route)),
    route!("GET" "/openapi.json" "status" "This document: OpenAPI 3.1, ETag is the sha256 of the body" Anonymous
        responses [200 => Json("OpenApiDocument"), 304 => Empty]
        => || get(crate::openapi::openapi_json_route)),
    route!("GET" "/docs" "status" "Redoc rendering of /openapi.json" Anonymous
        responses [200 => Html]
        => || get(|| async { super::static_page(include_str!("../../www/docs.html")) })),
    route!("GET" "/contract.json" "status" "The host.* tool contract; ETag is the contract sha" Anonymous
        responses [200 => Json("ContractDocument"), 304 => Empty]
        => || get(contract_json_route)),
    route!("GET" "/plans.json" "billing" "Plan catalog" Anonymous
        responses [200 => Json("Plans")]
        => || get(plans_json_route)),
    route!("GET" "/events.json" "status" "Event-kind registry" Anonymous
        responses [200 => Json("EventKinds")]
        => || get(events_json_route)),
    route!("GET" "/errors.json" "status" "Every error code the host can return (generated from the error registry)" Anonymous
        responses [200 => Json("ErrorCatalog")]
        => || get(errors_json_route)),
    route!("GET" "/capabilities.json" "status" "MCP capability matrix; implemented=1 keeps implemented rows" Anonymous
        query ["implemented"] request None; cors false;
        responses [200 => Json("Capabilities")]
        => || get(capabilities_json)),
    route!("GET" "/protocol/frames.json" "status" "Frames each request type may carry" Anonymous
        responses [200 => Json("FramesDocument")]
        => || get(protocol_frames_json)),
    route!("GET" "/help/{code}" "help" "Help page for an error code" Anonymous
        responses [200 => Html, 404 => Html]
        => || get(help_page)),
    route!("GET" "/help" "help" "Help index" Anonymous
        responses [200 => Html]
        => || get(help_index)),
    // ---- well-known ----------------------------------------------------------
    route!("GET" "/.well-known/mcp/{namespace}/server.json" "well-known" "Per-tenant MCP server card" Anonymous
        responses [200 => Json("ServerCard"), 404 => Empty]
        => || get(well_known_server_json)),
    route!("GET" "/.well-known/mcp/catalog.json" "well-known" "Public tool catalog" Anonymous
        responses [200 => Json("Catalog"), 500 => Json("LegacyError")]
        => || get(well_known_catalog)),
    route!("GET" "/.well-known/oauth-protected-resource" "well-known" "RFC 9728 protected-resource metadata" Anonymous
        responses [200 => Json("ProtectedResourceMetadata"), 500 => Json("LegacyError")]
        => || get(well_known_oauth_protected_resource)),
    // Equal to `oauth::entry_metadata_path()`; pinned by a unit test below.
    route!("GET" "/.well-known/oauth-protected-resource/login/mcp" "well-known" "Protected-resource metadata for the OAuth entry URL" Anonymous
        responses [200 => Json("ProtectedResourceMetadata")]
        => || get(well_known_oauth_protected_resource_entry)),
    route!("GET" "/.well-known/oauth-protected-resource/t/{namespace}/mcp" "well-known" "Per-tenant protected-resource metadata" Anonymous
        responses [200 => Json("ProtectedResourceMetadata"), 404 => Json("LegacyError")]
        => || get(well_known_oauth_protected_resource_tenant)),
    route!("GET" "/.well-known/oauth-authorization-server" "well-known" "RFC 8414 authorization-server metadata" Anonymous
        responses [200 => Json("AuthorizationServerMetadata")]
        => || get(well_known_authorization_server)),
    route!("GET" "/.well-known/openid-configuration" "well-known" "Same document as oauth-authorization-server" Anonymous
        responses [200 => Json("AuthorizationServerMetadata")]
        => || get(well_known_authorization_server)),
    route!("GET" "/.well-known/oauth-authorization-server/t/{namespace}/mcp" "well-known" "Per-tenant authorization-server metadata" Anonymous
        responses [200 => Json("AuthorizationServerMetadata"), 404 => Json("LegacyError")]
        => || get(well_known_authorization_server_tenant)),
    route!("GET" "/.well-known/openid-configuration/t/{namespace}/mcp" "well-known" "Same document as the per-tenant oauth-authorization-server" Anonymous
        responses [200 => Json("AuthorizationServerMetadata"), 404 => Json("LegacyError")]
        => || get(well_known_authorization_server_tenant)),
    route!("GET" "/.well-known/jwks.json" "well-known" "Signing keys for hosted access tokens" Anonymous
        responses [200 => Json("Jwks")]
        => || get(well_known_authz_jwks)),
    // ---- oauth ---------------------------------------------------------------
    route!("GET" "/oauth/authorize" "oauth" "Authorization endpoint (consent page)" Anonymous
        responses [200 => Html]
        => || get(crate::authz::get_authorize)),
    route!("POST" "/oauth/authorize" "oauth" "Consent approval" Anonymous
        responses [302 => Empty]
        => || post(crate::authz::post_authorize)),
    route!("GET" "/oauth/unverified-app" "oauth" "Explainer for the unverified-app caution" Anonymous
        responses [200 => Html]
        => || get(crate::authz::get_unverified_app_explainer)),
    route!("POST" "/oauth/register" "oauth" "Dynamic client registration" Anonymous
        responses [201 => Json("OpenObject"), 400 => Json("OpenObject")]
        => || post(crate::authz::post_register)),
    route!("POST" "/oauth/token" "oauth" "Token endpoint" Anonymous
        responses [200 => Json("OpenObject"), 400 => Json("OpenObject")]
        => || post(crate::authz::post_token)),
    route!("POST" "/oauth/revoke" "oauth" "Token revocation" Anonymous
        responses [200 => Empty]
        => || post(crate::authz::post_revoke)),
    route!("GET" "/oauth/federation/callback" "oauth" "Upstream identity-provider redirect target" Anonymous
        responses [200 => Html]
        => || get(crate::federation::get_callback)),
    route!("POST" "/oauth/federation/callback" "oauth" "Consent approval after upstream login" Anonymous
        responses [200 => Html]
        => || post(crate::federation::post_callback)),

    // ---- billing -------------------------------------------------------------
    route!("POST" "/billing/webhook" "billing" "Stripe-signed billing webhook" Signature
        responses [200 => Empty, 400 => Empty]
        => || post(billing_webhook)),
    route!("GET" "/billing/done" "billing" "Checkout success landing page" Anonymous
        responses [200 => Html]
        => || get(billing_done)),
    route!("GET" "/billing/cancel" "billing" "Checkout cancel landing page" Anonymous
        responses [200 => Html]
        => || get(billing_cancel)),
    // ---- receivers -----------------------------------------------------------
    route!("POST" "/hooks/{namespace}/{tool}" "webhooks" "Inbound event for a tenant tool" Token
        responses [200 => Json("OpenObject"), 404 => Empty]
        => || post(crate::hooks::hook_receive)),
    route!("POST" "/hook/{id}" "webhooks" "Opaque-id webhook trigger" Token
        responses [200 => Json("OpenObject"), 404 => Empty]
        => || post(crate::webhooks::hook_receive)),
    route!("POST" "/email/inbound/{namespace}" "webhooks" "Inbound mail delivery" Signature
        responses [200 => Json("OpenObject"), 404 => Empty]
        => || post(crate::email_inbound::http_inbound)),
    // ---- signed downloads and pages -------------------------------------------
    route!("GET" "/exports/{run_id}" "assets" "Signed tenant-export download" Token
        responses [200 => Binary, 404 => Empty]
        => || get(crate::export::download)),
    route!("GET" "/charts/{id}" "assets" "Signed chart share link" Token
        responses [200 => Html, 404 => Empty]
        => || get(crate::chart::download)),
    route!("GET" "/blobs/{id}" "assets" "Signed blob download" Token
        responses [200 => Binary, 404 => Empty]
        => || get(crate::blobs::download)),
    route!("GET" "/a/{id}" "assets" "Signed answer page for host.ask.create" Token
        responses [200 => Html, 404 => Empty]
        => || get(crate::asks::get_page)),
    route!("POST" "/a/{id}" "assets" "Submit an answer" Token
        responses [200 => Html, 404 => Empty]
        => || post(crate::asks::post_answer)),

    // ---- claim ---------------------------------------------------------------
    route!("GET" "/claim/{token}" "claim" "Human claim page" Token
        responses [200 => Html, 404 => Html]
        => || get(crate::claim::get_claim)),
    route!("POST" "/claim/{token}" "claim" "Submit the claim form" Token
        responses [200 => Html, 404 => Html]
        => || post(crate::claim::post_claim)),

    route!("GET" "/claim/methods.json" "claim" "Ownership verification methods this host knows, and which are available" Anonymous
        responses [200 => Json("ClaimMethods")]
        => || get(crate::verify::get_methods)),
    route!("GET" "/claim/github/callback" "claim" "GitHub OAuth callback that verifies the tenant owner" Anonymous
        query ["code", "state"] request None; cors false;
        responses [200 => Html, 400 => Html, 409 => Html, 502 => Html]
        => || get(crate::verify::get_github_callback)),
    route!("GET" "/claim/verify/{code}" "claim" "Magic-link verification" Token
        responses [200 => Html, 404 => Html]
        => || get(crate::claim::get_verify)),
    route!("GET" "/vault/connect/{token}" "claim" "Redeem a one-time upstream-token handoff link" Token
        responses [302 => Empty, 404 => Empty]
        => || get(crate::vault::get_connect)),
    route!("GET" "/vault/callback" "claim" "Upstream-token code exchange" Anonymous
        query ["code", "state"] request None; cors false;
        responses [200 => Html]
        => || get(crate::vault::get_callback)),
    // ---- onboarding ----------------------------------------------------------
    route!("GET" "/u/new" "onboarding" "One-button signup page" Anonymous
        responses [200 => Html]
        => || get(get_new_url)),
    route!("POST" "/u/new" "onboarding" "Mint a tenant and its first URL" Anonymous
        responses [200 => Html, 429 => Html]
        => || post(post_new_url)),

    route!("GET" "/connect" "onboarding" "One-click client install page" Anonymous
        responses [200 => Html]
        => || get(connect_page)),
    route!("GET" "/connect/go/{client}" "onboarding" "Client deep-link redirect" Anonymous
        responses [302 => Empty, 404 => Empty]
        => || get(connect_go)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_metadata_path_literal_matches_oauth_module() {
        assert!(ROUTES.iter().any(|r| r.path == crate::oauth::entry_metadata_path()));
    }

    #[test]
    fn method_path_pairs_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for r in ROUTES {
            assert!(seen.insert((r.method, r.path)), "duplicate {} {}", r.method, r.path);
        }
    }
}
