//! AC5 — Given a tenant on a plan with `listen_streams_max` N holding N open
//! streams, When it opens one more, Then that listen ends with
//! `listen_streams_max` and data `{limit: N, used: N, plan}`; and When the
//! client drops one stream, Then a new listen succeeds within 5 s, a publish
//! afterwards sends nothing to the dropped stream, the hub holds 0 entries for
//! the tenant once all streams close, and `admin.funnel`
//! `listen_refused_cap_24h` increments by one.

use crate::common;
use common::{ListenStream, McpClient, TestServer, signup};
use serde_json::{Value, json};
use std::time::Duration;

const N: i64 = 2;

async fn open_acked(base: &str, key: &str, id: u64) -> ListenStream {
    let mut stream = ListenStream::open(base, key, id).await;
    assert_eq!(stream.status, 200, "listen {id} is accepted");
    let (method, _) = stream.next_method(Duration::from_secs(5)).await.expect("acknowledgement");
    assert_eq!(method, "notifications/subscriptions/acknowledged");
    stream
}

/// The acknowledgement goes out before `listen` registers with the hub.
async fn wait_open(server: &TestServer, tenant_id: i64, want: usize) {
    for _ in 0..50 {
        if server.state.listen_hub.open_for(tenant_id) == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("hub holds {} streams, wanted {want}", server.state.listen_hub.open_for(tenant_id));
}

async fn refused_24h(server: &TestServer) -> i64 {
    let funnel = mcphost::admin::funnel(&server.state, &json!({})).await.expect("admin.funnel");
    funnel["listen_refused_cap_24h"].as_i64().expect("listen_refused_cap_24h is a number")
}

#[tokio::test]
async fn sublis_ac05_listen_beyond_the_plan_ceiling_is_refused_and_a_dropped_slot_frees() {
    let server = TestServer::start_with_plans(|plans| {
        for plan in plans.plans.iter_mut() {
            plan.listen_streams_max = N;
        }
    })
    .await;
    let (ns, key) = signup(&server.base_url, "sublis5").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("db").expect("tenant");
    let plan = tenant.plan.clone();
    assert_eq!(refused_24h(&server).await, 0);

    let mut first = open_acked(&server.base_url, &key, 1).await;
    let mut second = open_acked(&server.base_url, &key, 2).await;
    wait_open(&server, tenant.id, N as usize).await;

    // One more: acknowledged, then ended with the structured plan error.
    let mut over = ListenStream::open(&server.base_url, &key, 3).await;
    assert_eq!(over.status, 200);
    let mut last: Option<Value> = None;
    while let Some(msg) = over.next_message(Duration::from_secs(5)).await {
        last = Some(msg);
    }
    assert!(over.is_closed(), "the refused listen ends");
    let last = last.expect("a final message");
    assert_eq!(last["id"], json!(3), "{last}");
    assert_eq!(last["error"]["data"]["error_code"], json!("listen_streams_max"), "{last}");
    assert_eq!(last["error"]["data"]["limit"], json!(N), "{last}");
    assert_eq!(last["error"]["data"]["used"], json!(N), "{last}");
    assert_eq!(last["error"]["data"]["plan"], json!(plan), "{last}");
    assert_eq!(refused_24h(&server).await, 1, "the refusal is counted once");
    assert_eq!(server.state.listen_hub.open_for(tenant.id), N as usize, "a refused listen holds no slot");

    // Dropping one stream frees its slot within 5 s.
    second.close();
    drop(second);
    let mut third = None;
    for _ in 0..50 {
        if server.state.listen_hub.open_for(tenant.id) < N as usize {
            third = Some(open_acked(&server.base_url, &key, 4).await);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut third = third.expect("a new listen succeeds within 5 s of the drop");
    wait_open(&server, tenant.id, N as usize).await;
    assert_eq!(refused_24h(&server).await, 1, "a successful listen is not counted");

    // A publish reaches the live streams only; the dropped stream has no entry.
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool.publish", json!({"name": "ping", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish");
    for stream in [&mut first, &mut third] {
        let (method, _) = stream.next_method(Duration::from_secs(5)).await.expect("live stream is notified");
        assert_eq!(method, "notifications/tools/list_changed");
    }
    assert_eq!(server.state.listen_hub.open_for(tenant.id), N as usize, "only the two live streams hold slots");

    first.close();
    third.close();
    drop((first, third));
    for _ in 0..50 {
        if server.state.listen_hub.entries() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(server.state.listen_hub.entries(), 0, "the hub holds nothing once every stream closed");
}
