//! PRD-mcphost-agent-mesh-ops
//! AC9 (P1) -- Given tenant S received 50 `contact_refused` refusals in an
//! hour, When the admin calls `admin.mesh.stats(window="1h")`, Then
//! `refusals_by_code.contact_refused` for S is 50.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn ac9_stats_tallies_refusals_by_code_per_sender() {
    let server = TestServer::start().await;
    let (ns_s, key_s) = signup(&server.base_url, "Tenant S").await;
    let (ns_c, key_c) = signup(&server.base_url, "Tenant C").await;
    let client_s = McpClient::with_bearer(&server.base_url, &key_s);
    let client_c = McpClient::with_bearer(&server.base_url, &key_c);

    client_c
        .tools_call("host.agent.profile_set", json!({"contact_policy": "closed"}))
        .await
        .expect("C sets closed policy");

    for i in 0..50 {
        let raw = client_s
            .tools_call("host.msg.send", json!({"to": [ns_c.clone()], "body": format!("attempt {i}")}))
            .await
            .unwrap_or_else(|e| panic!("send {i} should not error at the top level: {e:?}"));
        let result = extract_structured(&raw);
        let refused = result["refused"].as_array().expect("refused array");
        assert_eq!(refused.len(), 1, "{result:?}");
        assert_eq!(refused[0]["code"], json!("contact_refused"), "{result:?}");
    }

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let raw = admin
        .tools_call("admin.mesh.stats", json!({"window": "1h"}))
        .await
        .expect("admin.mesh.stats");
    let stats = extract_structured(&raw);

    assert_eq!(
        stats["refusals_by_code"]["contact_refused"],
        json!(50),
        "{stats:?}"
    );

    let tenants = stats["tenants"].as_array().expect("tenants array");
    let s_entry = tenants
        .iter()
        .find(|t| t["tenant"] == json!(ns_s))
        .unwrap_or_else(|| panic!("S must appear in the per-tenant list: {tenants:?}"));
    assert_eq!(s_entry["refusals_by_code"]["contact_refused"], json!(50), "{s_entry:?}");
}
