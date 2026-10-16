//! PRD-mcphost-notify-user-channel-b
//! AC2 -- Given a free tenant that sent 20 notifies today, When the 21st is
//! sent, Then it fails `notify_limit` with `resets_unix`, and
//! `host.quickstart.limits.plan.notifies_per_day` reads 20 from `plans.rs`.

use crate::common;
use axum::{Router, routing::post};
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::plans::PlanCatalog;
use serde_json::json;

#[tokio::test]
async fn ntfyb_ac02() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ntfyb ac02 tenant").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(&agent.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    let from_plans_rs = PlanCatalog::default_catalog().get("free").expect("free plan").notifies_per_day;
    assert_eq!(from_plans_rs, 20, "the free plan's notifies_per_day is 20");
    assert_eq!(quickstart["limits"]["plan"]["notifies_per_day"], json!(from_plans_rs), "{quickstart}");

    let router = Router::new().route("/hook", post(|| async { "ok" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind fixture");
    let addr = listener.local_addr().expect("fixture addr");
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve fixture");
    });
    agent
        .tools_call("host.notify.channel_add", json!({"kind": "webhook", "target": format!("http://{addr}/hook")}))
        .await
        .expect("channel_add");

    for n in 0..20 {
        agent
            .tools_call("host.notify.send", json!({"text": format!("note {n}"), "level": "info"}))
            .await
            .unwrap_or_else(|e| panic!("notify {n}: {e:?}"));
    }
    let err = agent
        .tools_call("host.notify.send", json!({"text": "note 20", "level": "info"}))
        .await
        .expect_err("the 21st notify must be refused");
    assert_eq!(err.error_code.as_deref(), Some("notify_limit"), "{err:?}");
    assert!(err.data["resets_unix"].is_i64(), "{err:?}");
    assert_eq!(err.data["limit"], json!(20), "{err:?}");
}
