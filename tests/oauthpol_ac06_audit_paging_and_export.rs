//! PRD-mcphost-oauth-client-policy
//! AC6 (P0) — Given ACs 1-5, When `host.oauth.audit` is paged with `limit:
//! 2`, Then every event appears once across pages with `event`,
//! `client_id`, `reason` where applicable and a hashed IP; When
//! `host.oauth.audit_export` covers the window, Then the JSON lines equal
//! the paged rows; a window whose export exceeds 50 MiB returns
//! `export_too_large` with a suggested window.

use crate::common;
use common::{McpClient, TestServer, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

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

async fn register_dcr_client(http: &reqwest::Client, base_url: &str) -> String {
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    register["client_id"].as_str().expect("client_id").to_string()
}

/// Pages `host.oauth.audit` at `limit`, following `cursor` until
/// exhausted, returning every event in page order.
async fn page_all_audit(client: &McpClient, limit: i64) -> Vec<Value> {
    let mut events = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut args = json!({"limit": limit});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let page = client.tools_call("host.oauth.audit", args).await.expect("host.oauth.audit");
        let structured = common::extract_structured(&page);
        let page_events = structured["events"].as_array().cloned().unwrap_or_default();
        let is_empty = page_events.is_empty();
        events.extend(page_events);
        cursor = structured["cursor"].as_str().map(str::to_string);
        if cursor.is_none() || is_empty {
            break;
        }
    }
    events
}

#[tokio::test]
async fn every_event_pages_once_and_export_matches_or_refuses_too_large() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Audit Tenant").await;
    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let resource = format!("{}/mcp", server.base_url);

    // Generate a handful of distinct events: policy_change, an allowed
    // client reaching consent+token+refresh, and a refused one.
    tenant_client
        .tools_call("host.oauth.policy_set", json!({"clients": "allowlist", "allowlist": []}))
        .await
        .expect("policy_set");

    let refused_client = register_dcr_client(&http, &server.base_url).await;
    let refused = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", refused_client.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", code_challenge_for("verifier-for-refused-client-43-chars-min-ok").as_str()),
            ("code_challenge_method", "S256"),
            ("state", "refused"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(refused.headers().get("location").is_none());

    tenant_client
        .tools_call("host.oauth.policy_set", json!({"clients": "any"}))
        .await
        .expect("policy_set any");

    let allowed_client = register_dcr_client(&http, &server.base_url).await;
    let verifier = "verifier-for-allowed-client-at-least-43-chars-ok";
    let consent = http
        .post(format!("{}/oauth/authorize", server.base_url))
        .form(&[
            ("response_type", "code"),
            ("client_id", allowed_client.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", code_challenge_for(verifier).as_str()),
            ("code_challenge_method", "S256"),
            ("state", "allowed"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
            ("tenant_key", key.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent.status().is_redirection());
    let location = consent.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_base, query) = location.split_once('?').unwrap();
    let code = query_param(query, "code").expect("code present").to_string();
    let token_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", allowed_client.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    let refresh_token = token_resp["refresh_token"].as_str().expect("refresh_token").to_string();
    let _refresh_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token.as_str())])
        .send()
        .await
        .expect("POST /oauth/token refresh")
        .json()
        .await
        .expect("parse refresh response");

    tenant_client.tools_call("host.oauth.revoke_all", json!({})).await.expect("host.oauth.revoke_all");

    // Paged at limit:2, every event appears exactly once, in the same
    // order a single big page returns them.
    let paged = page_all_audit(&tenant_client, 2).await;
    let single_page = tenant_client
        .tools_call("host.oauth.audit", json!({"limit": 500}))
        .await
        .expect("host.oauth.audit single page");
    let all_at_once = common::extract_structured(&single_page)["events"].as_array().cloned().unwrap_or_default();
    assert_eq!(paged.len(), all_at_once.len(), "paging must not drop or duplicate events");
    assert_eq!(paged, all_at_once, "paged events must match a single large page, in order");

    let ids: BTreeSet<i64> = paged.iter().map(|e| e["id"].as_i64().unwrap()).collect();
    assert_eq!(ids.len(), paged.len(), "every event id must be distinct across pages");
    assert!(paged.iter().any(|e| e["event"] == json!("policy_change")));
    assert!(paged.iter().any(|e| e["event"] == json!("consent") && e["client_id"] == json!(allowed_client)));
    assert!(paged.iter().any(|e| e["event"] == json!("token")));
    assert!(paged.iter().any(|e| e["event"] == json!("refresh")));
    assert!(paged.iter().any(|e| e["event"] == json!("revoke")));
    let refused_event = paged
        .iter()
        .find(|e| e["client_id"] == json!(refused_client))
        .expect("the refused client's own audit row");
    assert_eq!(refused_event["reason"], json!("client_not_allowed"));
    assert!(
        paged.iter().any(|e| e["ip_hash"].is_string()),
        "at least the HTTP-originated events must carry a hashed IP: {paged:?}"
    );

    // audit_export over the same window returns the identical rows as
    // JSON lines.
    let since = paged.iter().map(|e| e["ts"].as_i64().unwrap()).min().unwrap() - 1;
    let until = paged.iter().map(|e| e["ts"].as_i64().unwrap()).max().unwrap() + 1;
    let export = tenant_client
        .tools_call("host.oauth.audit_export", json!({"since": since, "until": until}))
        .await
        .expect("host.oauth.audit_export");
    let lines = common::extract_structured(&export)["lines"].as_array().cloned().unwrap_or_default();
    let exported: Vec<Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l.as_str().expect("line is a string")).expect("line parses as JSON"))
        .collect();
    assert_eq!(exported, all_at_once, "export must equal the paged rows for the same window");

    // A window whose export exceeds 50 MiB is refused, naming a narrower
    // window to retry with.
    let (ns2, key2) = signup(&server.base_url, "Big Audit Tenant").await;
    let tenant2 = server.state.db.find_tenant_by_namespace(ns2).await.unwrap().expect("tenant2 row");
    server
        .state
        .db
        .insert_oauth_audit_rows_bulk_for_test(tenant2.id, 60, 1_000_000)
        .await
        .expect("seed oversized audit window");
    let big_client = McpClient::with_bearer(&server.base_url, &key2);
    let now = mcphost::state::now_unix();
    let err = big_client
        .tools_call("host.oauth.audit_export", json!({"since": now - 10, "until": now + 10}))
        .await
        .expect_err("a 60 MiB window must be refused");
    assert_eq!(err.data["error_code"], json!("export_too_large"));
    assert!(err.data["since"].is_i64() && err.data["until"].is_i64(), "a suggested window must be named: {:?}", err.data);
}
