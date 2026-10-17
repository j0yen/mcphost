//! PRD-mcphost-same-user-second-agent
//! AC1 — Given a fresh tenant and its `host.whoami.join_url`, When a second
//! connection on that URL calls `host.whoami` with no key, header, or claim
//! code, Then `namespace` equals the first tenant's, `client_id` differs from
//! client 0, and `clients` has 2 rows (the falsifier as a test).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn sibl_ac01_second_connection_joins_same_namespace() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Sibling Host").await;
    let first = McpClient::with_bearer(&server.base_url, &key);

    let me = extract_structured(&first.tools_call("host.whoami", json!({})).await.expect("whoami"));
    let join_url = me["join_url"].as_str().expect("whoami carries join_url").to_string();
    assert!(join_url.contains("/j/") && join_url.ends_with("/mcp"), "join_url shape: {join_url}");
    let client0 = me["client_id"].as_str().expect("client_id").to_string();
    assert_eq!(me["clients"].as_array().expect("clients").len(), 1);

    // The second connection: only the join URL -- no key, header or claim code.
    let path = join_url.trim_start_matches(&server.base_url).to_string();
    let second = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let joined = extract_structured(&second.tools_call("host.whoami", json!({})).await.expect("join whoami"));

    assert_eq!(joined["namespace"], json!(ns), "the sibling lands in the first tenant: {joined}");
    let sibling = joined["client_id"].as_str().expect("client_id");
    assert_ne!(sibling, client0, "the sibling has a client id of its own");
    let clients = joined["clients"].as_array().expect("clients");
    assert_eq!(clients.len(), 2, "two client rows: {joined}");
    assert_eq!(
        clients.iter().filter(|c| c["this"] == json!(true)).count(),
        1,
        "exactly one row is `this`: {joined}"
    );
    assert!(
        clients.iter().any(|c| c["client_id"] == json!(sibling) && c["this"] == json!(true)),
        "`this` is the sibling: {joined}"
    );
    let onboarding = &joined["onboarding"];
    assert_eq!(onboarding["joined"], json!(ns));
    assert_eq!(onboarding["client_id"], json!(sibling));
    let own_url = onboarding["url"].as_str().expect("onboarding.url");
    assert!(own_url.contains("/u/") && own_url.ends_with("/mcp"), "own URL: {own_url}");

    // The sibling's own URL works with no key and resolves to the same tenant.
    let own_path = own_url.trim_start_matches(&server.base_url).to_string();
    let via_own = McpClient::new(&server.base_url).with_path(&own_path);
    let again = extract_structured(&via_own.tools_call("host.whoami", json!({})).await.expect("own url whoami"));
    assert_eq!(again["namespace"], json!(ns));
    assert_eq!(again["client_id"], json!(sibling));

    // The first client sees the sibling too, and is still `this` itself.
    let me_again = extract_structured(&first.tools_call("host.whoami", json!({})).await.expect("whoami"));
    assert_eq!(me_again["clients"].as_array().expect("clients").len(), 2);
    assert_eq!(me_again["client_id"], json!(client0));
}
