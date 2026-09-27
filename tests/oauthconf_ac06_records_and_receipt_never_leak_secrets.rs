//! PRD-mcphost-oauth-conformance-harness
//! AC6 (P0) — Given the probe's records and receipt from AC5, When
//! searched, Then no key, code, access token or refresh token substring
//! appears.
//!
//! This PRD's own tree never reaches a real code/token exchange (every
//! family but `prm`/`iss` stops at `read_as_metadata`, `unsupported`), so a
//! search over the *actual gate/CLI output* would be vacuous -- there is
//! nothing to leak yet. This test instead drives the simulator through a
//! real authorize -> consent -> token round trip against the fake AS (same
//! pattern as AC4's `issue_a_fresh_code`), so real secret VALUES exist,
//! then serializes the resulting [`oauthclient::ScenarioResult`]s and
//! [`mcphost::cli::oauth_probe::render_receipt`] output -- the exact two
//! shapes AC5's `--json` and `--receipt` print -- and greps for every one
//! of those secret values.

use crate::fake_as;
use crate::oauthclient;
use fake_as::FakeAsOptions;
use mcphost::cli::oauth_probe::render_receipt;
use oauthclient::{ClientKind, ScenarioResult, TokenRequestArgs, Verdict};

const CLIENT_ID: &str = "https://simulator.example/client-metadata.json";

#[tokio::test]
async fn full_flow_records_and_receipt_never_carry_a_secret_substring() {
    let http = reqwest::Client::new();
    let as_server = fake_as::start(FakeAsOptions { cimd_supported: true, none_auth_method: true, registration_endpoint: false }).await;
    let (meta_record, meta) = oauthclient::read_as_metadata(&http, &as_server.base_url).await;
    assert_eq!(meta_record.verdict, Verdict::Pass, "AS metadata must be readable: {meta_record:?}");
    let meta = meta.expect("AsMetadata");
    let authorization_endpoint = meta.authorization_endpoint.clone().expect("authorization_endpoint");
    let token_endpoint = meta.token_endpoint.clone().expect("token_endpoint");

    let req = oauthclient::build_authorize_request(
        &authorization_endpoint,
        CLIENT_ID,
        ClientKind::Claude,
        &as_server.base_url,
        Some("mcp"),
        &[],
    );
    let (authorize_record, pending) = oauthclient::submit_authorize(&http, &req).await;
    assert_eq!(authorize_record.verdict, Verdict::Pass, "authorize must pass: {authorize_record:?}");
    let pending = pending.expect("PendingConsent");
    let claim_code = pending.claim_code.clone();

    let (consent_record, response) = oauthclient::consent(&http, &pending).await;
    assert_eq!(consent_record.verdict, Verdict::Pass, "consent must pass: {consent_record:?}");
    let response = response.expect("AuthorizeResponse");
    let auth_code = response.code.clone();

    let iss_record = oauthclient::iss_check(&response, &meta.issuer);

    let (token_record, token_result) = oauthclient::token(
        &http,
        TokenRequestArgs {
            token_endpoint: &token_endpoint,
            code: &response.code,
            code_verifier: Some(&req.code_verifier),
            redirect_uri: req.params.get("redirect_uri").unwrap(),
            client_id: CLIENT_ID,
            state: None,
        },
    )
    .await;
    assert_eq!(token_record.verdict, Verdict::Pass, "token exchange must pass: {token_record:?}");
    let token_result = token_result.expect("TokenResult");
    let access_token = token_result.access_token.clone().expect("access_token");
    let refresh_token = token_result.refresh_token.clone().expect("refresh_token");

    // A replay of the same code -- the AS refuses it, but this step's own
    // request/response still touched the code and verifier, so it belongs
    // in the same leak-surface check as every other step.
    let replay_record = oauthclient::attack_replay(
        &http,
        TokenRequestArgs {
            token_endpoint: &token_endpoint,
            code: &response.code,
            code_verifier: Some(&req.code_verifier),
            redirect_uri: req.params.get("redirect_uri").unwrap(),
            client_id: CLIENT_ID,
            state: None,
        },
    )
    .await;

    let results = vec![ScenarioResult {
        name: "ac06_full_flow_probe".to_string(),
        family: "pkce".to_string(),
        verdict: token_record.verdict,
        records: vec![authorize_record, consent_record, iss_record, token_record, replay_record],
    }];

    // The exact serialization AC5's `--json` prints.
    let json_out = serde_json::to_string_pretty(&results).expect("serialize results");
    // The exact serialization AC5's `--receipt` writes.
    let receipt_out = render_receipt(&results);

    let secrets: &[(&str, &str)] = &[
        ("key/claim_code", &claim_code),
        ("code", &auth_code),
        ("code_verifier", &req.code_verifier),
        ("access_token", &access_token),
        ("refresh_token", &refresh_token),
    ];
    for (label, secret) in secrets {
        let secret: &str = secret;
        assert!(!secret.is_empty(), "test setup: {label} must be a real, non-empty value");
        assert!(
            !json_out.contains(secret),
            "--json output must never contain the {label} value {secret:?}: {json_out}"
        );
        assert!(
            !receipt_out.contains(secret),
            "--receipt output must never contain the {label} value {secret:?}: {receipt_out}"
        );
    }
}
