//! PRD-mcphost-second-session-nudge
//! AC1 (P0) — Given a claimed tenant with `first_call_unix` 30 h ago, no
//! `second_session_unix`, no `nudged_unix`, and a verified email, When the
//! daily sweep runs, Then exactly one `return` email is sent with the
//! remember string, tool count and the one URL, and the row has
//! `nudged_unix` set and `nudge_channel = email`.

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
async fn claimed_tenant_30h_silent_gets_exactly_one_return_email() {
    let fake = Arc::new(mcphost::email::FakeEmailClient::new());
    let server = TestServer::start_with_email(fake.clone()).await;
    let anon = McpClient::new(&server.base_url);

    let signup = extract_structured(
        &anon
            .tools_call(
                "signup",
                json!({"name": "AC1 Tenant", "remember": "cleaning Joe's CSVs"}),
            )
            .await
            .expect("signup"),
    );
    let namespace = signup["tenant"].as_str().expect("tenant").to_string();
    let key = signup["key"].as_str().expect("key").to_string();
    let claim_token =
        token_from_claim_url(signup["claim_url"].as_str().expect("claim_url")).to_string();

    let tenant_client = McpClient::with_bearer(&server.base_url, &key);
    tenant_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");

    // Claim and verify -- the one real path that sets `owner_email`/
    // `owner_verified_at` (same flow as mcphost_human_claim_magic_link_ac03).
    let http = reqwest::Client::new();
    http.post(format!("{}/claim/{claim_token}", server.base_url))
        .form(&[("email", "ac1@b.co")])
        .send()
        .await
        .expect("POST /claim/{token}");
    let claim_sends = fake.sends();
    assert_eq!(claim_sends.len(), 1, "claim email should have sent: {claim_sends:?}");
    let code = code_from_email_body(&claim_sends[0].text_body).to_string();
    let verify_resp = http
        .get(format!("{}/claim/verify/{code}", server.base_url))
        .send()
        .await
        .expect("GET /claim/verify/{code}");
    assert_eq!(verify_resp.status(), reqwest::StatusCode::OK);

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");
    assert!(tenant.owner_verified_at.is_some(), "tenant must be claimed+verified");
    assert!(tenant.first_call_unix.is_some(), "publish call must have stamped first_call_unix");

    // Backdate first_call_unix 30h, as if the tenant's only call happened a
    // day and a quarter ago.
    let backdated = mcphost::state::now_unix() - 30 * 3600;
    server
        .state
        .db
        .set_tenant_stamp_for_test(tenant.id, "first_call_unix", backdated)
        .await
        .expect("backdate first_call_unix");

    let report = mcphost::returns::sweep(&server.state).await.expect("sweep");
    assert_eq!(report.emailed, 1, "{report:?}");
    assert_eq!(report.selected, 1, "{report:?}");

    let sends = fake.sends();
    assert_eq!(sends.len(), 2, "claim email + exactly one return email: {sends:?}");
    let return_email = &sends[1];
    assert_eq!(return_email.to, "ac1@b.co");
    assert!(
        return_email.text_body.contains("cleaning Joe's CSVs"),
        "body must carry the remember string: {}",
        return_email.text_body
    );
    assert!(
        return_email.text_body.contains("Tools published: 1"),
        "body must carry the published tool count: {}",
        return_email.text_body
    );
    assert!(
        return_email.text_body.contains(&server.base_url),
        "body must carry the one URL: {}",
        return_email.text_body
    );

    let (nudged_unix, nudge_channel, nudge_attempts, _second_session_unix) = server
        .state
        .db
        .nudge_status(tenant.id)
        .await
        .expect("nudge_status")
        .expect("tenant exists");
    assert!(nudged_unix.is_some(), "nudged_unix must be set");
    assert_eq!(nudge_channel.as_deref(), Some("email"));
    assert_eq!(nudge_attempts, 0, "a successful send never touched the failure counter");
}
