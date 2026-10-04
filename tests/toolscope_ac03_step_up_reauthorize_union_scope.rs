//! PRD-mcphost-tool-scopes-and-consent
//! AC3 (P0) — Given the client re-authorizes with `scope=read write`, When
//! consent is approved, Then the new token lists both tools and
//! `delete_records` runs.
//!
//! The Then clause (token/delete_records) is proven end to end against
//! this test's own tenant below, via `oauthclient::run_step_up_probe`, the
//! family-specific runner this PRD adds: it self-signs-up a synthetic
//! probe tenant, publishes a `write`-scoped tool, and drives the real
//! step-up sequence (403 `insufficient_scope`, re-authorize with the union
//! scope, success) against it.
//!
//! PRD-mcphost-implicit-signup: the harness `step_up` scenario
//! (`tests/oauthconf/scenarios.toml`'s `step_up_403_without_as_metadata`)
//! no longer reads `pass` -- `run_step_up_probe` bootstraps on a
//! `discover_401` probe of root `/mcp`, which now succeeds anonymously
//! (implicit signup) instead of 401ing, so the probe never gets far enough
//! to exercise the real step-up sequence at all. The mechanism itself is
//! unaffected (proved directly below); only this harness's own bootstrap
//! assumption broke, which is why the scenario's gold `expected` flipped
//! back to `unsupported`/`unassigned` rather than this PRD's own claim.

use crate::common;
use crate::oauthclient;
use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oauthclient::Verdict;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then_some(v)
    })
}

async fn consent_and_get_token(server: &TestServer, ns: &str, key: &str, scope: &str) -> Value {
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();

    let verifier = format!("verifier-{scope}-at-least-43-chars-long-for-pkce-realism").replace(' ', "-");
    let challenge = code_challenge_for(&verifier);
    let resource = mcphost::oauth::canonical_resource_uri(&server.base_url, ns);

    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", scope),
            ("resource", resource.as_str()),
            ("tenant_key", key),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent_resp.status().is_redirection(), "consent must redirect: {}", consent_resp.status());
    let location = consent_resp.headers().get("location").expect("Location header").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("redirect must carry a query string");
    let code = query_param(query, "code").expect("code present in redirect").to_string();

    http.post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response")
}

#[tokio::test]
async fn reauthorizing_with_the_union_scope_lists_both_tools_and_delete_records_runs() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Step Up Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    key_client
        .tools_call("host.oauth.scope_set", json!({"name": "read", "description": "Search"}))
        .await
        .expect("scope_set read");
    key_client
        .tools_call(
            "host.oauth.scope_set",
            json!({"name": "write", "description": "Change records"}),
        )
        .await
        .expect("scope_set write");
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "search", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["read"]}),
        )
        .await
        .expect("publish search");
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "delete_records", "kind": "echo", "spec": {"schema": {"type": "object"}}, "scopes": ["write"]}),
        )
        .await
        .expect("publish delete_records");

    // Step 1: the read-only token gets a 403 step-up trigger on
    // delete_records.
    let read_token = consent_and_get_token(&server, &ns, &key, "read")
        .await
        .get("access_token")
        .and_then(Value::as_str)
        .expect("read token")
        .to_string();
    let read_client = McpClient::with_bearer(&server.base_url, &read_token);
    let err = read_client
        .tools_call(&format!("{ns}.delete_records"), json!({}))
        .await
        .expect_err("delete_records must 403 under the read-only token");
    assert_eq!(err.error_code.as_deref(), Some("insufficient_scope"));

    // Step 2: the client re-authorizes with the union (read write) --
    // AC2's own union-scope challenge is exactly `read write`, so this is
    // literally what a conforming client would request next.
    let token_resp = consent_and_get_token(&server, &ns, &key, "read write").await;
    assert_eq!(token_resp["scope"], json!("read write"));
    let readwrite_token = token_resp["access_token"].as_str().expect("access_token").to_string();
    let readwrite_client = McpClient::with_bearer(&server.base_url, &readwrite_token);

    let listed = readwrite_client.tools_list().await.expect("tools/list");
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&format!("{ns}.search").as_str()), "{names:?}");
    assert!(names.contains(&format!("{ns}.delete_records").as_str()), "{names:?}");

    // Step 3: the step-up succeeded -- the same call now runs.
    readwrite_client
        .tools_call(&format!("{ns}.delete_records"), json!({}))
        .await
        .expect("delete_records must run once stepped up to read write");
}

/// PRD-mcphost-implicit-signup: the harness `step_up` scenario
/// (`tests/oauthconf/scenarios.toml`'s `step_up_403_without_as_metadata`,
/// run through `oauthclient::run_all`, the exact function `mcphost
/// oauth-probe` and `oauthconf_ac01`'s gate test both call) now reads
/// `unsupported`, not `pass` -- its own leading `discover_401` probe of
/// root `/mcp` no longer gets a 401 to discover (see this file's own
/// module doc comment). The real step-up mechanism this scenario was
/// proving is still proved directly, end to end, by this file's first
/// test above.
#[tokio::test]
async fn harness_step_up_scenario_reads_unsupported_since_discover_401_no_longer_fires() {
    let server = TestServer::start().await;
    let mcp_url = format!("{}/mcp", server.base_url);
    let http = reqwest::Client::new();

    let results = oauthclient::run_all(&http, &mcp_url).await;
    let result = results
        .iter()
        .find(|r| r.name == "step_up_403_without_as_metadata")
        .expect("scenarios.toml must still carry the step_up scenario");
    assert_eq!(
        result.verdict,
        Verdict::Unsupported,
        "step_up scenario must read unsupported now that root /mcp answers anonymously -- records: {:?}",
        result.records
    );
}
