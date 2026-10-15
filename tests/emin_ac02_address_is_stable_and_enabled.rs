//! PRD-mcphost-inbound-email-address-per-tenant
//! AC2 -- Given a tenant, When it calls `host.email.address`, Then it receives
//! `{address: "<tenant>@in.mcphost.dev", enabled: true}` and the same address
//! on every later call.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::email::EmailProvider;
use serde_json::json;

#[tokio::test]
async fn emin_ac02_address_is_the_tenant_slug_and_stable() {
    let server = TestServer::start_with_email_inbound("whsec-test", EmailProvider::Resend, false).await;
    let (ns, key) = signup(&server.base_url, "emin tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let first = extract_structured(&client.tools_call("host.email.address", json!({})).await.expect("address"));
    assert_eq!(first["address"], json!(format!("{ns}@in.mcphost.dev")), "{first}");
    assert_eq!(first["domain"], json!("in.mcphost.dev"), "{first}");
    assert_eq!(first["enabled"], json!(true), "{first}");

    for _ in 0..2 {
        let again = extract_structured(&client.tools_call("host.email.address", json!({})).await.expect("address"));
        assert_eq!(again, first, "the address is the same on every later call");
    }
}

#[tokio::test]
async fn emin_ac02_disabled_without_a_secret() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "emin tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let v = extract_structured(&client.tools_call("host.email.address", json!({})).await.expect("address"));
    assert_eq!(v["address"], json!(format!("{ns}@in.mcphost.dev")), "{v}");
    assert_eq!(v["enabled"], json!(false), "{v}");
}
