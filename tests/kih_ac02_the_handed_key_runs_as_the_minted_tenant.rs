//! PRD-mcphost-implicit-signup-key-in-hand
//! AC2 (P0) -- Given AC 1's sessionless client inside the 600 s window, When
//! it calls `host.table.list` with `tenant_key` set to the
//! `onboarding.tenant_key` value, Then the call runs as the minted tenant and
//! lists its table, the peer record for that IP and User-Agent is forgotten
//! (`frees_peer`), and no `tenant_key_missing` error is produced.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::peer_recent::peer_key;
use serde_json::json;

const UA: &str = "kih-ac02/agent";

#[tokio::test]
async fn kih_ac02() {
    let server = TestServer::start().await;
    let peer = peer_key("127.0.0.1", Some(UA));
    let now = mcphost::state::now_unix();

    let client = McpClient::new(&server.base_url).with_user_agent(UA);
    let created = extract_structured(
        &client
            .tools_call("host.table.create", json!({"name": "runs", "columns": {"id": "integer"}}))
            .await
            .expect("first keyless call is an implicit signup"),
    );
    let namespace = created["onboarding"]["tenant"].as_str().expect("onboarding.tenant").to_string();
    let key = created["onboarding"]["tenant_key"].as_str().expect("onboarding.tenant_key").to_string();
    let recorded = server.state.redeemed_peers.lookup(&peer, now).expect("the sessionless mint is remembered for the peer");
    assert_eq!(recorded.0, namespace, "the record names the minted tenant");

    // The second call, with the handed key and still no session.
    let listed = extract_structured(
        &client
            .tools_call("host.table.list", json!({"tenant_key": key}))
            .await
            .expect("the handed key works: no tenant_key_missing"),
    );
    let tables = listed["tables"].as_array().expect("tables array");
    assert!(tables.iter().any(|t| t["name"] == json!("runs")), "it ran as the minted tenant: {listed}");
    assert!(
        server.state.redeemed_peers.lookup(&peer, mcphost::state::now_unix()).is_none(),
        "the peer record is forgotten once the key is in use"
    );
    assert_eq!(server.state.db.list_tenants().await.expect("tenants").len(), 1, "no second tenant was minted");
}
