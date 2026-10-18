//! AC5 (P0) -- PRD-mcphost-resources-for-tenant-data. Given a free tenant with
//! 8 open subscriptions, When the 9th `resources/subscribe` arrives, Then
//! `resources_max` is returned with `data.limit: 8`; Given the connection
//! closes, Then all 8 entries are gone and a new connection may open 8 again.

use crate::common::{TestServer, signup};
use crate::mres_ac04_subscribe_gets_updated_on_append::{SessionClient, serve_sessions};
use serde_json::json;
use std::time::Duration;

fn uri(n: usize) -> String {
    format!("mcphost://table/t{n}")
}

#[tokio::test]
async fn mres_ac05_ninth_subscribe_is_resources_max_and_closing_frees_all_eight() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "mres ac05").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("query").expect("tenant");
    assert_eq!(tenant.plan, "free");
    // Requirement 5: both ceilings ride `ceilings_json`, hence quickstart's `limits.plan`.
    let ceilings = server.state.plans.get("free").expect("free plan").ceilings_json();
    assert_eq!(ceilings["resources_max"], 8, "{ceilings}");
    assert_eq!(ceilings["resource_read_bytes_max"], 1_048_576, "{ceilings}");
    assert_eq!(server.state.plans.get("pro").expect("pro plan").ceilings_json()["resources_max"], 64);
    let url = serve_sessions(&server).await;

    let mut first = SessionClient::open(&url, &key).await;
    for n in 0..8 {
        let reply = first.rpc("resources/subscribe", json!({"uri": uri(n)})).await;
        assert!(reply.get("result").is_some(), "subscription {n}: {reply}");
    }
    assert_eq!(server.state.resource_subs.open_for(tenant.id), 8);

    let ninth = first.rpc("resources/subscribe", json!({"uri": uri(8)})).await;
    assert_eq!(ninth["error"]["data"]["error_code"], "resources_max", "{ninth}");
    assert_eq!(ninth["error"]["data"]["limit"], 8, "{ninth}");
    assert_eq!(server.state.resource_subs.open_for(tenant.id), 8, "a refused subscribe adds nothing");

    // Re-subscribing a URI already held is idempotent, not a ninth entry.
    let again = first.rpc("resources/subscribe", json!({"uri": uri(0)})).await;
    assert!(again.get("result").is_some(), "{again}");

    first.close().await;
    // The session's service winds down asynchronously after the DELETE.
    let mut open = server.state.resource_subs.open_for(tenant.id);
    for _ in 0..40 {
        if open == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        open = server.state.resource_subs.open_for(tenant.id);
    }
    assert_eq!(open, 0, "closing the connection drops all of its entries");

    let mut second = SessionClient::open(&url, &key).await;
    for n in 0..8 {
        let reply = second.rpc("resources/subscribe", json!({"uri": uri(n)})).await;
        assert!(reply.get("result").is_some(), "new connection, subscription {n}: {reply}");
    }
    assert_eq!(server.state.resource_subs.open_for(tenant.id), 8);
}
