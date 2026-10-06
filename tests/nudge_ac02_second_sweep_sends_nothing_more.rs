//! PRD-mcphost-second-session-nudge
//! AC2 (P0) — Given the same tenant as AC1 (already nudged), When the
//! sweep runs again the next day, Then nothing is sent and the row is
//! unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;
use std::sync::Arc;

fn token_from_claim_url(claim_url: &str) -> &str {
    claim_url.rsplit('/').next().expect("claim_url has a path segment")
}

fn code_from_email_body(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in claim email body");
    let after = &body[idx + "/claim/verify/".len()..];
    after.split_whitespace().next().expect("code token")
}

#[tokio::test]
async fn second_sweep_leaves_an_already_nudged_tenant_untouched() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let signup = extract_structured(
        &anon
            .tools_call("signup", json!({"name": "AC2 Tenant"}))
            .await
            .expect("signup"),
    );
    let namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let key = signup["key"].as_str().expect("key").to_string();
    let claim_token =
        token_from_claim_url(signup["claim_url"].as_str().expect("claim_url")).to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client.tools_call("host.whoami", json!({})).await.expect("whoami");

    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "ac2@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let claim_sends = fake.sends();
    let code = code_from_email_body(&claim_sends[0].text_body).to_string();
    http.get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("find tenant")
        .expect("tenant exists");
    let backdated = mcphost::state::now_unix() - 30 * 3600;
    server
        .state
        .db
        .set_tenant_stamp_for_test(tenant.id, "first_call_unix", backdated)
        .await
        .expect("backdate first_call_unix");

    let first_report = mcphost::returns::sweep(&server.state).await.expect("first sweep");
    assert_eq!(first_report.emailed, 1, "{first_report:?}");
    let after_first = server
        .state
        .db
        .nudge_status(tenant.id)
        .await
        .expect("nudge_status")
        .expect("tenant exists");

    // "the next day": first_call_unix is still 30h in the past, nothing
    // about the row changes on its own -- the second sweep call stands in
    // for the next day's run, same compression `sched_ac1`'s own
    // `tick_once` test uses instead of a real wall-clock wait.
    let second_report = mcphost::returns::sweep(&server.state).await.expect("second sweep");
    assert_eq!(second_report.selected, 0, "{second_report:?}");
    assert_eq!(second_report.emailed, 0, "{second_report:?}");

    let after_second = server
        .state
        .db
        .nudge_status(tenant.id)
        .await
        .expect("nudge_status")
        .expect("tenant exists");
    assert_eq!(after_first, after_second, "row must be unchanged by the second sweep");

    let sends = fake.sends();
    assert_eq!(sends.len(), 2, "claim email + the one AC1-equivalent return email, no more: {sends:?}");
}
