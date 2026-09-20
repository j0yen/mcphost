//! PRD-mcphost-agent-mesh-ops
//! AC1 (P0) -- Given 40 messages between four real tenants and 200 between
//! synthetic ones in the last hour, When the admin calls
//! `admin.mesh.stats(window="1h")`, Then `messages` reads `{real: 40,
//! synthetic: 200}` and the per-tenant list is ordered by volume with the
//! top synthetic sender first.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup_with_synthetic_header};
use serde_json::json;

async fn send_n(client: &McpClient, to: &str, n: usize) {
    for i in 0..n {
        client
            .tools_call("host.msg.send", json!({"to": [to], "body": format!("msg {i}")}))
            .await
            .unwrap_or_else(|e| panic!("send {i} to {to} should succeed: {e:?}"));
    }
}

/// This suite's `TestServer` is a real TCP listener, so every connection to
/// it is `127.0.0.1` (loopback) -- `control::signup`'s own classification
/// (`state::classify_source_class`) treats every loopback signup as
/// synthetic (`harness:unstamped`) regardless of headers, same as
/// `tests/attrib_ac4_external_signup_counts_real.rs`'s own doc comment
/// explains. A genuinely "real" (external, `synthetic: None`) tenant is
/// only reachable by calling `control::signup` directly with a fake
/// non-loopback source IP, bypassing the HTTP layer for this one seam.
async fn signup_real(state: &mcphost::state::AppState, base_url: &str, name: &str) -> (String, McpClient) {
    let result = mcphost::control::signup(
        state,
        &json!({"name": name}),
        "8.8.8.8",
        mcphost::control::SignupAttribution::default(),
    )
    .await
    .expect("real (external) signup");
    let ns = result["tenant"].as_str().expect("tenant field").to_string();
    let key = result["key"].as_str().expect("key field").to_string();
    (ns.clone(), McpClient::with_bearer(base_url, &key))
}

#[tokio::test]
async fn ac1_stats_splits_real_and_synthetic_and_ranks_top_synthetic_sender_first() {
    let server = TestServer::start_with_signup_rate_limit(100).await;

    // Four real tenants: 10 sends each direction between two pairs = 40.
    let (ns_r1, client_r1) = signup_real(&server.state, &server.base_url, "Real One").await;
    let (ns_r2, client_r2) = signup_real(&server.state, &server.base_url, "Real Two").await;
    let (ns_r3, client_r3) = signup_real(&server.state, &server.base_url, "Real Three").await;
    let (ns_r4, client_r4) = signup_real(&server.state, &server.base_url, "Real Four").await;
    send_n(&client_r1, &ns_r2, 10).await;
    send_n(&client_r2, &ns_r1, 10).await;
    send_n(&client_r3, &ns_r4, 10).await;
    send_n(&client_r4, &ns_r3, 10).await;

    // Four synthetic tenants: 59 + 55 + 55 + 31 = 200, each under the free
    // plan's 60 msgs/hour quota, with S1 uniquely the top sender.
    let s1 = signup_with_synthetic_header(&server.base_url, "Synth One", "synthorg:meshops-ac1").await;
    let s2 = signup_with_synthetic_header(&server.base_url, "Synth Two", "synthorg:meshops-ac1").await;
    let s3 = signup_with_synthetic_header(&server.base_url, "Synth Three", "synthorg:meshops-ac1").await;
    let s4 = signup_with_synthetic_header(&server.base_url, "Synth Four", "synthorg:meshops-ac1").await;
    let ns_s1 = s1["tenant"].as_str().expect("s1 tenant").to_string();
    let ns_s2 = s2["tenant"].as_str().expect("s2 tenant").to_string();
    let ns_s3 = s3["tenant"].as_str().expect("s3 tenant").to_string();
    let ns_s4 = s4["tenant"].as_str().expect("s4 tenant").to_string();
    let client_s1 = McpClient::with_bearer(&server.base_url, s1["key"].as_str().expect("s1 key"));
    let client_s2 = McpClient::with_bearer(&server.base_url, s2["key"].as_str().expect("s2 key"));
    let client_s3 = McpClient::with_bearer(&server.base_url, s3["key"].as_str().expect("s3 key"));
    let client_s4 = McpClient::with_bearer(&server.base_url, s4["key"].as_str().expect("s4 key"));
    send_n(&client_s1, &ns_s2, 59).await;
    send_n(&client_s2, &ns_s1, 55).await;
    send_n(&client_s3, &ns_s4, 55).await;
    send_n(&client_s4, &ns_s3, 31).await;

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let raw = admin
        .tools_call("admin.mesh.stats", json!({"window": "1h"}))
        .await
        .expect("admin.mesh.stats");
    let stats = extract_structured(&raw);

    assert_eq!(stats["messages"], json!({"real": 40, "synthetic": 200}), "{stats:?}");

    let tenants = stats["tenants"].as_array().expect("tenants array");
    assert!(!tenants.is_empty(), "{tenants:?}");
    assert_eq!(tenants[0]["tenant"], json!(ns_s1), "{tenants:?}");
    assert_eq!(tenants[0]["messages"], json!(59), "{tenants:?}");
    assert!(!tenants[0]["synthetic"].is_null(), "{tenants:?}");

    // Ordered by volume, descending.
    let mut prev = i64::MAX;
    for t in tenants {
        let cnt = t["messages"].as_i64().expect("messages count");
        assert!(cnt <= prev, "not sorted descending: {tenants:?}");
        prev = cnt;
    }
}
