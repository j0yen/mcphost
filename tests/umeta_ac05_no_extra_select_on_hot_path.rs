//! PRD-mcphost-usage-in-every-response AC5 -- Given the test harness's SQL
//! query counter, When one successful call runs, Then the number of `SELECT`
//! statements equals the pre-PRD number for the same call.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::db::Db;
use serde_json::json;

/// `SELECT` statements one successful echo call ran before this PRD,
/// measured on the pre-PRD tree with this same counter and call.
const PRE_PRD_SELECTS_PER_CALL: u64 = 4;

/// `SELECT` statements one successful echo call runs when its tenant has ONE
/// secret stored: the no-secret budget, plus the `get_secret` query
/// `build_secret_map` has always run per stored secret, plus the one
/// `secret_hosts` query that loads the secrets' host bindings
/// (PRD-mcphost-secret-non-exfiltration-proof R1). A tenant with no secrets
/// must never pay either extra query, and a tenant with secrets must not pay
/// a second binding query.
const SELECTS_WITH_SECRETS: u64 = 6; // 4 + get_secret + secret_hosts

/// `Db::select_count` is process-global: two measuring tests running at once
/// would each count the other's SELECTs in every window, so the measurements
/// take turns.
static MEASURING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Publish an echo tool (optionally after storing one bound secret), warm
/// it, and return the minimum `SELECT` count over five successful calls.
async fn min_selects_per_call(with_secret: bool) -> (u64, Vec<u64>) {
    let _turn = MEASURING.lock().await;
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Meta Selects").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    if with_secret {
        client
            .tools_call(
                "host.secret.set",
                json!({"name": "api_key", "value": "s3cret-value-1234", "hosts": ["api.example.com"]}),
            )
            .await
            .expect("set secret");
    }
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    let qualified = format!("{ns}.hello");
    // Warm every lazy first-call path (onboarding hints, first_own_call) so
    // the measured calls are steady-state.
    client.tools_call(&qualified, json!({"warm": true})).await.expect("warm");
    client.tools_call(&qualified, json!({"warm": true})).await.expect("warm");

    server.state.db.enable_select_counter();
    // Background tasks can add a SELECT inside one window; a real extra
    // query on the call path is in every window, so the minimum is the
    // call's own cost.
    let mut per_call = Vec::new();
    for n in 0..5 {
        let before = Db::select_count();
        let result = client.tools_call(&qualified, json!({"n": n})).await.expect("call");
        assert!(result["_meta"]["usage"]["calls_per_day"]["used"].is_number(), "{result}");
        per_call.push(Db::select_count() - before);
    }
    let min = per_call.iter().copied().min().expect("samples");
    (min, per_call)
}

#[tokio::test]
async fn successful_call_runs_the_pre_prd_number_of_selects() {
    let (min, per_call) = min_selects_per_call(false).await;
    assert_eq!(min, PRE_PRD_SELECTS_PER_CALL, "SELECTs per successful call: {per_call:?}");
}

#[tokio::test]
async fn successful_call_with_a_secret_runs_the_named_budget_of_selects() {
    let (min, per_call) = min_selects_per_call(true).await;
    assert_eq!(min, SELECTS_WITH_SECRETS, "SELECTs per successful call with a secret: {per_call:?}");
}
