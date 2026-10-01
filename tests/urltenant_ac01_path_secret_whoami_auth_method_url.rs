//! PRD-mcphost-url-bound-tenants
//! AC1 (P0) — Given a tenant with a URL secret, When a streamable-HTTP
//! `tools/call host.whoami` is POSTed to `/u/<secret>/mcp` with no
//! `Authorization` header and no `tenant_key`, Then the result names that
//! tenant with `auth_method: "url"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn path_secret_alone_authenticates_as_that_tenant() {
    let server = TestServer::start().await;

    // Sign up and mint a URL secret for this tenant on the same session
    // (host.key_rotate needs no tenant_key here since the connection that
    // ran signup is already bound to the tenant it created).
    let session = McpClient::new(&server.base_url).with_session_continuity();
    let signed_up = session
        .tools_call("signup", json!({"name": "AC1 Tenant"}))
        .await
        .expect("signup");
    let namespace = extract_structured(&signed_up)["tenant"]
        .as_str()
        .expect("tenant namespace")
        .to_string();

    let rotated = session
        .tools_call("host.key_rotate", json!({}))
        .await
        .expect("host.key_rotate on the bound session");
    let rotated = extract_structured(&rotated);
    let url = rotated["url"].as_str().expect("key_rotate returns a url").to_string();
    assert!(
        url.starts_with(&format!("{}/u/", server.base_url)) && url.ends_with("/mcp"),
        "url must be a /u/<secret>/mcp link: {url}"
    );
    let path = url.trim_start_matches(&server.base_url).to_string();

    // A fresh, credential-less client that only knows the URL.
    let url_client = McpClient::new(&server.base_url).with_path(&path);
    let whoami = url_client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami over the URL secret alone, no Authorization, no tenant_key");
    let whoami = extract_structured(&whoami);

    assert_eq!(whoami["tenant"], json!(namespace), "{whoami}");
    assert_eq!(whoami["auth_method"], json!("url"), "{whoami}");
}
