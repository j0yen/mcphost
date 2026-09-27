//! PRD-mcphost-hosted-authorization-server
//! AC9 (P0) — Given the logs, `admin_audit` rows and `host.runs.get`
//! output produced by ACs 4-8, When searched, Then no substring of any
//! tenant key, code, access token, refresh token or client secret
//! appears.
//!
//! `TestServer` runs the host in-process on a tokio task (see
//! `tests/common/mod.rs`), not as a subprocess, so there is no separate
//! log stream to grep here. What a tenant or operator can actually read
//! back after the fact is `admin.audit_log`, `host.runs.list`/
//! `host.runs.get` (which inlines a run's stored result -- though AC4-8's
//! host.* control tools never carry a run id, so these come back empty),
//! and each tenant-tool call's own RPC result -- the same surface
//! `tests/handoff_ac06_errors_never_echo_token_or_key.rs` checks for a
//! different secret. This test exercises the AC4-8 flows (a confidential
//! and a public client, a full grant, a code replay, a refresh-token
//! rotation reuse, and both revocation paths) end to end, then asserts
//! none of the tenant key, codes, access tokens, refresh tokens or client
//! secret produced along the way appear anywhere in those trails. The
//! direct OAuth HTTP responses that legitimately hand a secret to its
//! owner (register/authorize/token) are deliberately excluded from the
//! search -- that is how the caller is supposed to learn it.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use serde_json::{Value, json};

const NATIVE_REDIRECT_URI: &str = "http://127.0.0.1/cb";
const WEB_REDIRECT_URI: &str = "https://client.example/cb";

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

struct GrantOutcome {
    code: String,
    access_token: String,
    refresh_token: String,
}

#[allow(clippy::too_many_arguments)]
async fn complete_grant(
    http: &reqwest::Client,
    server: &TestServer,
    client_id: &str,
    client_secret: Option<&str>,
    key: &str,
    verifier: &str,
) -> GrantOutcome {
    let challenge = code_challenge_for(verifier);
    let resource = format!("{}/mcp", server.base_url);
    let redirect_uri = if client_secret.is_some() { WEB_REDIRECT_URI } else { NATIVE_REDIRECT_URI };
    let consent_resp = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    let location = consent_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').unwrap();
    let code = query_param(query, "code").unwrap().to_string();

    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    let token: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&form)
        .send()
        .await
        .expect("token exchange")
        .json()
        .await
        .expect("parse token response");
    GrantOutcome {
        code,
        access_token: token["access_token"].as_str().unwrap().to_string(),
        refresh_token: token["refresh_token"].as_str().unwrap().to_string(),
    }
}

#[tokio::test]
async fn ac4_through_ac8_flows_leave_no_secret_in_audit_or_runs() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Leak Check Tenant").await;
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let mut secrets: Vec<String> = vec![key.clone()];
    // Every tenant-tool-call result produced along the way -- host.* control
    // tools carry `run_id: None` (see `handler.rs`'s dispatch: an ordinary
    // synchronous call is never a job) so they never populate `host.runs.*`
    // or `host.tool_logs` (those are for actual sandboxed/job executions),
    // making direct RPC results the trail worth scanning for these tools.
    let mut tool_responses: Vec<Value> = Vec::new();

    let native_register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": [NATIVE_REDIRECT_URI]}))
        .send()
        .await
        .expect("POST /oauth/register (native)")
        .json()
        .await
        .expect("parse native register response");
    let native_client_id = native_register["client_id"].as_str().unwrap().to_string();

    let web_register: Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "web", "redirect_uris": [WEB_REDIRECT_URI], "client_name": "Web Client"}))
        .send()
        .await
        .expect("POST /oauth/register (web)")
        .json()
        .await
        .expect("parse web register response");
    let web_client_id = web_register["client_id"].as_str().unwrap().to_string();
    let web_client_secret = web_register["client_secret"].as_str().expect("web client gets a secret").to_string();
    secrets.push(web_client_secret.clone());

    // AC4: a full grant via the native client, then hosted-bearer calls.
    let grant_1 = complete_grant(&http, &server, &native_client_id, None, &key, "grant-one-verifier-at-least-43-chars-long").await;
    secrets.push(grant_1.code.clone());
    secrets.push(grant_1.access_token.clone());
    secrets.push(grant_1.refresh_token.clone());
    let bearer_1 = McpClient::with_bearer(&server.base_url, &grant_1.access_token);
    let r = bearer_1.tools_call("host.state.set", json!({"key": "hosted", "value": "yes"})).await.expect("host.state.set");
    tool_responses.push(common::extract_structured(&r));
    let r = bearer_1.tools_call("host.state.get", json!({"key": "hosted"})).await.expect("host.state.get");
    tool_responses.push(common::extract_structured(&r));
    let r = bearer_1.tools_call("host.whoami", json!({})).await.expect("host.whoami");
    tool_responses.push(common::extract_structured(&r));

    // AC5: replay grant_1's code -- revokes the whole grant.
    let replay_resp = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", grant_1.code.as_str()),
            ("redirect_uri", NATIVE_REDIRECT_URI),
            ("client_id", native_client_id.as_str()),
            ("code_verifier", "grant-one-verifier-at-least-43-chars-long"),
        ])
        .send()
        .await
        .expect("replay attempt");
    assert_eq!(replay_resp.status(), reqwest::StatusCode::BAD_REQUEST);

    // AC4/AC7: a full grant via the confidential (web) client, then rotate
    // its refresh token once and reuse the now-stale one.
    let grant_2 =
        complete_grant(&http, &server, &web_client_id, Some(&web_client_secret), &key, "grant-two-verifier-at-least-43-chars-long").await;
    secrets.push(grant_2.code.clone());
    secrets.push(grant_2.access_token.clone());
    secrets.push(grant_2.refresh_token.clone());

    let rotated: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", grant_2.refresh_token.as_str()),
            ("client_id", web_client_id.as_str()),
            ("client_secret", web_client_secret.as_str()),
        ])
        .send()
        .await
        .expect("refresh rotation")
        .json()
        .await
        .expect("parse rotation response");
    secrets.push(rotated["access_token"].as_str().unwrap().to_string());
    secrets.push(rotated["refresh_token"].as_str().unwrap().to_string());

    let reuse_resp = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", grant_2.refresh_token.as_str()),
            ("client_id", web_client_id.as_str()),
            ("client_secret", web_client_secret.as_str()),
        ])
        .send()
        .await
        .expect("refresh reuse attempt");
    assert_eq!(reuse_resp.status(), reqwest::StatusCode::BAD_REQUEST);

    // AC8: a third grant, listed then revoked via the tenant tool.
    let grant_3 = complete_grant(&http, &server, &native_client_id, None, &key, "grant-three-verifier-at-least-43-chars").await;
    secrets.push(grant_3.code.clone());
    secrets.push(grant_3.access_token.clone());
    secrets.push(grant_3.refresh_token.clone());

    // grant_1 (replayed) and grant_2 (refresh-reused) are both revoked by
    // now, so grant_3 is the only row `host.oauth.grants` still lists.
    let grants_result = key_client.tools_call("host.oauth.grants", json!({})).await.expect("host.oauth.grants");
    let grants = common::extract_structured(&grants_result);
    let grants_arr = grants["grants"].as_array().expect("grants array");
    assert_eq!(grants_arr.len(), 1, "grants: {grants}");
    let grant_3_id = grants_arr[0]["id"].as_i64().expect("grant_3's id");
    tool_responses.push(grants.clone());
    let r = key_client
        .tools_call("host.oauth.grant_revoke", json!({"id": grant_3_id}))
        .await
        .expect("host.oauth.grant_revoke");
    tool_responses.push(common::extract_structured(&r));

    // AC8: a fourth grant, revoked via `POST /oauth/revoke` with its
    // refresh token.
    let grant_4 = complete_grant(&http, &server, &native_client_id, None, &key, "grant-four-verifier-at-least-43-chars-x").await;
    secrets.push(grant_4.code.clone());
    secrets.push(grant_4.access_token.clone());
    secrets.push(grant_4.refresh_token.clone());
    let revoke_resp = http
        .post(format!("{}/oauth/revoke", server.base_url))
        .form(&[("token", grant_4.refresh_token.as_str())])
        .send()
        .await
        .expect("POST /oauth/revoke");
    assert_eq!(revoke_resp.status(), reqwest::StatusCode::OK);

    // Now gather every trail a tenant or operator could actually read
    // back. `host.runs.list`/`host.runs.get` (list(), then get() -- which
    // inlines the result list() itself leaves null) cover any run the
    // flow above happened to create (none of AC4-8's host.* control tools
    // carry a run id, so this is expected to come back empty -- scanned
    // anyway in case that ever changes), `admin.audit_log` covers the
    // grant revocation, and `tool_responses` covers every RPC result the
    // flow's own tenant-tool calls returned.
    let runs_list_result = key_client.tools_call("host.runs.list", json!({"limit": 200})).await.expect("host.runs.list");
    let runs_list = common::extract_structured(&runs_list_result);
    let run_ids: Vec<String> = runs_list["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .filter_map(|r| r["run_id"].as_str().map(str::to_string))
        .collect();

    let mut run_gets = Vec::new();
    for run_id in &run_ids {
        let got = key_client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("host.runs.get");
        run_gets.push(common::extract_structured(&got));
    }

    let admin_client = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let audit_result = admin_client.tools_call("admin.audit_log", json!({"limit": 500})).await.expect("admin.audit_log");
    let audit = common::extract_structured(&audit_result);

    let haystack = format!(
        "{}\n{}\n{}\n{}",
        serde_json::to_string(&runs_list).unwrap(),
        serde_json::to_string(&run_gets).unwrap(),
        serde_json::to_string(&audit).unwrap(),
        serde_json::to_string(&tool_responses).unwrap(),
    );

    for secret in &secrets {
        assert!(
            !haystack.contains(secret.as_str()),
            "secret {secret:?} must not appear in host.runs.list/get, admin_audit, or tenant-tool-call output"
        );
    }
}
