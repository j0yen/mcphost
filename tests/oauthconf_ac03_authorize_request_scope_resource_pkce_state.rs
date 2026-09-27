//! PRD-mcphost-oauth-conformance-harness
//! AC3 (P0) — Given a host answering 401 with `scope="mcp"`, When the
//! simulator authorizes with `--client claude`, Then the request carries
//! `scope=mcp offline_access`, `resource=<canonical url>`,
//! `code_challenge_method=S256` and a `state`; Given a 401 without
//! `scope`, Then scopes come from PRM `scopes_supported`.

use crate::oauthclient;
use oauthclient::{ClientKind, Verdict};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mock_host(challenge_scope: Option<&str>, prm_scopes_supported: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    let metadata_url = format!("{}/.well-known/oauth-protected-resource", server.uri());
    let challenge = match challenge_scope {
        Some(scope) => format!("Bearer resource_metadata=\"{metadata_url}\", scope=\"{scope}\""),
        None => format!("Bearer resource_metadata=\"{metadata_url}\""),
    };
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401).insert_header("WWW-Authenticate", challenge.as_str()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "resource": server.uri(),
            "authorization_servers": [],
            "scopes_supported": prm_scopes_supported,
        })))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn scope_resource_pkce_and_state_come_from_the_401_challenge() {
    let server = mock_host(Some("mcp"), &["ignored-when-challenge-has-scope"]).await;
    let http = reqwest::Client::new();

    let (discover_record, discover) = oauthclient::discover_401(&http, &format!("{}/mcp", server.uri())).await;
    assert_eq!(discover_record.verdict, Verdict::Pass, "discover_401 must pass: {discover_record:?}");
    let discover = discover.expect("Discover401Result");
    assert_eq!(discover.scope.as_deref(), Some("mcp"));

    let (prm_record, prm) = oauthclient::read_prm(&http, discover.resource_metadata_url.as_deref().unwrap()).await;
    assert_eq!(prm_record.verdict, Verdict::Pass, "read_prm must pass: {prm_record:?}");
    let prm = prm.expect("PrmDoc");

    let req = oauthclient::build_authorize_request(
        "https://as.example/authorize",
        "test-client-id",
        ClientKind::Claude,
        &prm.resource,
        discover.scope.as_deref(),
        &prm.scopes_supported,
    );

    assert_eq!(req.params.get("scope").map(String::as_str), Some("mcp offline_access"));
    assert_eq!(req.params.get("resource").map(String::as_str), Some(prm.resource.as_str()));
    assert_eq!(req.params.get("code_challenge_method").map(String::as_str), Some("S256"));
    assert!(req.params.get("code_challenge").is_some_and(|c| !c.is_empty()));
    assert!(req.params.get("state").is_some_and(|s| !s.is_empty()));
    assert_eq!(req.state, *req.params.get("state").unwrap());
    assert!(!req.code_verifier.is_empty());
}

#[tokio::test]
async fn scopes_fall_back_to_prm_scopes_supported_when_the_401_has_none() {
    let server = mock_host(None, &["prm-scope-a", "prm-scope-b"]).await;
    let http = reqwest::Client::new();

    let (discover_record, discover) = oauthclient::discover_401(&http, &format!("{}/mcp", server.uri())).await;
    assert_eq!(discover_record.verdict, Verdict::Pass, "discover_401 must pass: {discover_record:?}");
    let discover = discover.expect("Discover401Result");
    assert!(discover.scope.is_none(), "this challenge must carry no scope param");

    let (prm_record, prm) = oauthclient::read_prm(&http, discover.resource_metadata_url.as_deref().unwrap()).await;
    assert_eq!(prm_record.verdict, Verdict::Pass, "read_prm must pass: {prm_record:?}");
    let prm = prm.expect("PrmDoc");
    assert_eq!(prm.scopes_supported, vec!["prm-scope-a".to_string(), "prm-scope-b".to_string()]);

    let req = oauthclient::build_authorize_request(
        "https://as.example/authorize",
        "test-client-id",
        ClientKind::Claude,
        &prm.resource,
        discover.scope.as_deref(),
        &prm.scopes_supported,
    );

    assert_eq!(req.params.get("scope").map(String::as_str), Some("prm-scope-a prm-scope-b offline_access"));
}
