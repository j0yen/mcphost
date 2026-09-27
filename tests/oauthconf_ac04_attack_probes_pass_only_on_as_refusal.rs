//! PRD-mcphost-oauth-conformance-harness
//! AC4 (P0) — Given the fake AS issues a code, When the simulator replays
//! it, downgrades PKCE, tampers `state`, or redeems it as another client,
//! Then each attack probe records the AS's response and reads `pass` only
//! when the response is an error with no token.

use crate::fake_as;
use crate::oauthclient;
use fake_as::FakeAsOptions;
use oauthclient::{ClientKind, TokenRequestArgs, Verdict};

const CLIENT_ID: &str = "https://simulator.example/client-metadata.json";
const OTHER_CLIENT_ID: &str = "https://simulator.example/other-client-metadata.json";

/// Drives a full, legitimate authorize -> consent -> token round trip
/// against the fake AS, returning everything a follow-up attack probe
/// needs to redeem (or attack a redemption of) the same code.
async fn issue_a_fresh_code(
    http: &reqwest::Client,
    as_base_url: &str,
    meta: &oauthclient::AsMetadata,
) -> (oauthclient::AuthorizeRequest, oauthclient::AuthorizeResponse) {
    let authorization_endpoint = meta.authorization_endpoint.clone().expect("authorization_endpoint");
    let req = oauthclient::build_authorize_request(
        &authorization_endpoint,
        CLIENT_ID,
        ClientKind::Claude,
        as_base_url,
        Some("mcp"),
        &[],
    );
    let (submit_record, pending) = oauthclient::submit_authorize(http, &req).await;
    assert_eq!(submit_record.verdict, Verdict::Pass, "authorize must pass: {submit_record:?}");
    let pending = pending.expect("PendingConsent");

    let (consent_record, response) = oauthclient::consent(http, &pending).await;
    assert_eq!(consent_record.verdict, Verdict::Pass, "consent must pass: {consent_record:?}");
    let response = response.expect("AuthorizeResponse");
    assert_eq!(response.state, req.state, "the AS must echo back the state the request sent");

    let iss_record = oauthclient::iss_check(&response, &meta.issuer);
    assert_eq!(iss_record.verdict, Verdict::Pass, "iss_check must pass: {iss_record:?}");

    (req, response)
}

#[tokio::test]
async fn legit_redemption_succeeds_then_each_attack_probe_is_refused() {
    let http = reqwest::Client::new();
    let as_server = fake_as::start(FakeAsOptions { cimd_supported: true, none_auth_method: true, registration_endpoint: false }).await;
    let (meta_record, meta) = oauthclient::read_as_metadata(&http, &as_server.base_url).await;
    assert_eq!(meta_record.verdict, Verdict::Pass, "AS metadata must be readable: {meta_record:?}");
    let meta = meta.expect("AsMetadata");
    let token_endpoint = meta.token_endpoint.clone().expect("token_endpoint");

    // Happy path: a legitimate redemption succeeds and the resulting
    // access token actually admits a call -- proves the fake AS's PKCE/
    // state/client checks don't ALSO reject a correct request.
    let (req1, response1) = issue_a_fresh_code(&http, &as_server.base_url, &meta).await;
    let (token_record, token_result) = oauthclient::token(
        &http,
        TokenRequestArgs {
            token_endpoint: &token_endpoint,
            code: &response1.code,
            code_verifier: Some(&req1.code_verifier),
            redirect_uri: req1.params.get("redirect_uri").unwrap(),
            client_id: CLIENT_ID,
            state: None,
        },
    )
    .await;
    assert_eq!(token_record.verdict, Verdict::Pass, "legit redemption must pass: {token_record:?}");
    assert!(token_result.expect("TokenResult").access_token.is_some());

    // Attack 1: replay the SAME code+verifier a second time.
    let replay_record = oauthclient::attack_replay(
        &http,
        TokenRequestArgs {
            token_endpoint: &token_endpoint,
            code: &response1.code,
            code_verifier: Some(&req1.code_verifier),
            redirect_uri: req1.params.get("redirect_uri").unwrap(),
            client_id: CLIENT_ID,
            state: None,
        },
    )
    .await;
    assert_eq!(replay_record.verdict, Verdict::Pass, "replay must be refused: {replay_record:?}");

    // Attack 2: a fresh code, redeemed with no code_verifier (PKCE
    // downgrade).
    let (req2, response2) = issue_a_fresh_code(&http, &as_server.base_url, &meta).await;
    let pkce_record = oauthclient::attack_pkce_downgrade(
        &http,
        TokenRequestArgs {
            token_endpoint: &token_endpoint,
            code: &response2.code,
            code_verifier: Some(&req2.code_verifier),
            redirect_uri: req2.params.get("redirect_uri").unwrap(),
            client_id: CLIENT_ID,
            state: None,
        },
    )
    .await;
    assert_eq!(pkce_record.verdict, Verdict::Pass, "PKCE downgrade must be refused: {pkce_record:?}");

    // Attack 3: a fresh code, redeemed with a tampered state.
    let (req3, response3) = issue_a_fresh_code(&http, &as_server.base_url, &meta).await;
    let state_record = oauthclient::attack_state_tamper(
        &http,
        TokenRequestArgs {
            token_endpoint: &token_endpoint,
            code: &response3.code,
            code_verifier: Some(&req3.code_verifier),
            redirect_uri: req3.params.get("redirect_uri").unwrap(),
            client_id: CLIENT_ID,
            state: None,
        },
        "attacker-supplied-state-does-not-match",
    )
    .await;
    assert_eq!(state_record.verdict, Verdict::Pass, "state tamper must be refused: {state_record:?}");

    // Attack 4: a fresh code, redeemed as a different client.
    let (req4, response4) = issue_a_fresh_code(&http, &as_server.base_url, &meta).await;
    let cross_client_record = oauthclient::attack_cross_client_redeem(
        &http,
        TokenRequestArgs {
            token_endpoint: &token_endpoint,
            code: &response4.code,
            code_verifier: Some(&req4.code_verifier),
            redirect_uri: req4.params.get("redirect_uri").unwrap(),
            client_id: CLIENT_ID,
            state: None,
        },
        OTHER_CLIENT_ID,
    )
    .await;
    assert_eq!(cross_client_record.verdict, Verdict::Pass, "cross-client redemption must be refused: {cross_client_record:?}");
}
