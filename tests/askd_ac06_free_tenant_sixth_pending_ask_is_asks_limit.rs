//! PRD-mcphost-ask-user-decision
//! AC6 -- Given a free tenant with 5 pending asks, When `host.ask.create`
//! runs, Then it fails `asks_limit`, and
//! `host.quickstart.limits.plan.asks_open_max` reads 5 from `plans.rs`.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::plans::PlanCatalog;
use serde_json::json;

fn ask(n: usize) -> serde_json::Value {
    json!({"question": format!("question {n}?"), "options": ["yes", "no"]})
}

#[tokio::test]
async fn sixth_pending_ask_on_the_free_plan_fails_asks_limit() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "askd ac06 tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    let from_plans_rs = PlanCatalog::default_catalog().get("free").expect("free plan").asks_open_max;
    assert_eq!(from_plans_rs, 5, "the free plan's asks_open_max is 5");
    assert_eq!(quickstart["limits"]["plan"]["asks_open_max"], json!(from_plans_rs), "{quickstart}");

    for n in 0..5 {
        client.tools_call("host.ask.create", ask(n)).await.unwrap_or_else(|e| panic!("ask {n}: {e:?}"));
    }
    let err = client.tools_call("host.ask.create", ask(5)).await.expect_err("the sixth pending ask must be refused");
    assert_eq!(err.error_code.as_deref(), Some("asks_limit"), "{err:?}");
    assert_eq!(err.data["limit"], json!(5), "{err:?}");
    assert_eq!(err.data["next_call"], json!("host.ask.list"), "{err:?}");
    assert_eq!(err.data["next_args"], json!({"status": "pending"}), "{err:?}");
}
