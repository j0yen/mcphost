//! PRD-mcphost-federated-end-user-login
//! AC3 (P0) — Given the exchanged token, When it calls a python-kind tool
//! on `/t/acme/mcp`, Then the tool sees `MCPHOST_END_USER_ID` namespaced
//! with the provider issuer, `MCPHOST_END_USER_METHOD=federated`,
//! `MCPHOST_END_USER_EMAIL=u1@acme.test`, `MCPHOST_END_USER_NAME=U One`,
//! and `host.enduser.whoami` reports the same.

use crate::common;
use crate::federation;

use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use federation::{query_param, sign_id_token, KID_1};
use mcphost::sandbox;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Duration;

fn code_challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

#[tokio::test]
async fn federated_bearer_reaches_python_env_and_whoami() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Acme").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import os\n\
            def main(args):\n\
            \treturn {\n\
            \t    \"id\": os.environ.get(\"MCPHOST_END_USER_ID\"),\n\
            \t    \"method\": os.environ.get(\"MCPHOST_END_USER_METHOD\"),\n\
            \t    \"email\": os.environ.get(\"MCPHOST_END_USER_EMAIL\"),\n\
            \t    \"name\": os.environ.get(\"MCPHOST_END_USER_NAME\"),\n\
            \t}\n",
        "args_schema": {"type": "object"},
    });
    key_client
        .tools_call("host.tool_publish", json!({"name": "whoami_tool", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let provider = federation::start().await;
    key_client
        .tools_call(
            "host.oauth.provider_set",
            json!({"issuer": provider.issuer(), "client_id": "acme-client", "client_secret": "acme-secret"}),
        )
        .await
        .expect("provider_set must succeed");

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let register: serde_json::Value = http
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();

    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = code_challenge_for(verifier);
    let resource = format!("{}/t/{}/mcp", server.base_url, ns);

    let authorize_resp = http
        .get(format!("{}/oauth/authorize", server.base_url))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "abc"),
            ("scope", "mcp"),
            ("resource", resource.as_str()),
        ])
        .send()
        .await
        .expect("GET /oauth/authorize");
    let location = authorize_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').unwrap();
    let upstream_nonce = query_param(query, "nonce").unwrap().to_string();
    let upstream_state = query_param(query, "state").unwrap().to_string();

    let id_token = sign_id_token(
        KID_1,
        federation::priv_pem_1(),
        &provider.issuer(),
        "acme-client",
        "u1",
        &upstream_nonce,
        Some("u1@acme.test"),
        Some(true),
        Some("U One"),
        300,
    );
    federation::mount_token(&provider, &id_token).await;
    http.get(format!("{}/oauth/federation/callback", server.base_url))
        .query(&[("code", "upstream-code"), ("state", upstream_state.as_str())])
        .send()
        .await
        .expect("GET /oauth/federation/callback");
    let approve_resp = http
        .post(format!("{}/oauth/federation/callback", server.base_url))
        .form(&[("token", upstream_state.as_str())])
        .send()
        .await
        .expect("POST /oauth/federation/callback");
    let approve_location = approve_resp.headers().get("location").unwrap().to_str().unwrap().to_string();
    let (_, approve_query) = approve_location.split_once('?').unwrap();
    let code = query_param(approve_query, "code").unwrap().to_string();

    let token_resp: serde_json::Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    let access_token = token_resp["access_token"].as_str().expect("access_token").to_string();

    let bearer_client = McpClient::with_bearer(&server.base_url, &access_token);
    let expected_id = format!("{}#u1", provider.issuer());

    let result = poll_until_ready(&bearer_client, &format!("{ns}.whoami_tool"), json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(structured["id"], json!(expected_id), "{structured}");
    assert_eq!(structured["method"], json!("federated"), "{structured}");
    assert_eq!(structured["email"], json!("u1@acme.test"), "{structured}");
    assert_eq!(structured["name"], json!("U One"), "{structured}");

    let whoami_result = bearer_client.tools_call("host.enduser.whoami", json!({})).await.expect("whoami must succeed");
    let whoami = extract_structured(&whoami_result);
    assert_eq!(whoami["subject"], json!(expected_id), "{whoami}");
    assert_eq!(whoami["method"], json!("federated"), "{whoami}");
    assert_eq!(whoami["issuer"], json!(provider.issuer()), "{whoami}");
    assert_eq!(whoami["email"], json!("u1@acme.test"), "{whoami}");
    assert_eq!(whoami["name"], json!("U One"), "{whoami}");
}
