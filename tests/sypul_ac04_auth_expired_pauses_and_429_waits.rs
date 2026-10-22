//! PRD-mcphost-sync-pull-with-change-detection AC4 -- Given a `google_sheet`
//! sync whose fixture answers 401, When the run completes, Then `status:
//! auth_expired`, the sync is paused, exactly one inbox message names
//! `host.connect.start(provider="google")`, and three further ticks write no
//! receipt; Given 429 with `Retry-After: 120`, Then `next_run_unix` is now +
//! 120 s and the receipt error is `upstream returned HTTP 429`.

use crate::common::sypul::Fixture;
use crate::common::{McpClient, TestServer, extract_structured, signup};
use mcphost::sync::SourceKind;
use serde_json::{Value, json};

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool}: {e:?}")))
}

async fn make_due(server: &TestServer, sync_id: &str) {
    let now = mcphost::state::now_unix();
    mcphost::sync::set_next_run_for_test(&server.state, sync_id, now - 60).await.expect("move into the past");
}

#[tokio::test]
async fn sypul_ac04_a_401_pauses_with_one_notice_and_a_429_waits_retry_after() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "SYPUL AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("tenant lookup").expect("tenant");
    let (enc, nonce) = server.state.secrets.encrypt("ya29.fixture-token").expect("encrypt");
    server
        .state
        .db
        .upsert_tenant_vault_token(
            tenant.id,
            "google".to_string(),
            enc,
            nonce,
            None,
            None,
            mcphost::state::now_unix() + 3600,
            "spreadsheets.readonly".to_string(),
        )
        .await
        .expect("connect google");
    let fixture = Fixture::start(r#"{"values": [["id", "name"], ["1", "a"]]}"#, false).await;
    fixture.set_content_type("application/json");
    mcphost::sync::sources::set_api_base_for_test(&server.state, SourceKind::GoogleSheet, &fixture.base);

    // 401: auth_expired, paused, one notice, nothing more on later ticks.
    fixture.set_status(401, None);
    let created = call(
        &client,
        "host.sync.create",
        json!({"source": {"kind": "google_sheet", "ref": "sheet-one/Orders"}, "table": "sheet_one", "primary_key": "id"}),
    )
    .await;
    let sync_id = created["sync_id"].as_str().expect("sync_id").to_string();
    make_due(&server, &sync_id).await;
    mcphost::triggers::tick_once(&server.state).await.expect("tick");
    let got = call(&client, "host.sync.get", json!({"sync_id": sync_id})).await;
    assert_eq!(got["last_run"]["status"], "auth_expired", "{got}");
    assert_eq!(got["status"], "paused", "{got}");
    let inbox = call(&client, "host.msg.inbox", json!({})).await;
    let messages = inbox["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "{inbox}");
    assert!(
        messages[0]["body"].as_str().expect("body").contains(r#"host.connect.start(provider="google")"#),
        "{inbox}"
    );
    for _ in 0..3 {
        make_due(&server, &sync_id).await;
        mcphost::triggers::tick_once(&server.state).await.expect("further tick");
    }
    assert_eq!(mcphost::sync::receipt_count_for_test(&server.state, &sync_id).await.expect("count"), 1);
    let inbox = call(&client, "host.msg.inbox", json!({})).await;
    assert_eq!(inbox["messages"].as_array().expect("messages").len(), 1, "still one notice: {inbox}");

    // 429 Retry-After: 120 waits exactly that long and keeps the sync active.
    fixture.set_status(429, Some(120));
    let created = call(
        &client,
        "host.sync.create",
        json!({"source": {"kind": "google_sheet", "ref": "sheet-two/Orders"}, "table": "sheet_two", "primary_key": "id"}),
    )
    .await;
    let limited_id = created["sync_id"].as_str().expect("sync_id").to_string();
    make_due(&server, &limited_id).await;
    let before = mcphost::state::now_unix();
    mcphost::triggers::tick_once(&server.state).await.expect("tick");
    let after = mcphost::state::now_unix();
    let got = call(&client, "host.sync.get", json!({"sync_id": limited_id})).await;
    assert_eq!(got["last_run"]["status"], "rate_limited", "{got}");
    assert_eq!(got["last_run"]["error"], "upstream returned HTTP 429", "{got}");
    assert_eq!(got["status"], "active", "{got}");
    let next = got["next_run_unix"].as_i64().expect("next_run_unix");
    assert!(next >= before + 120 && next <= after + 120, "next_run_unix {next} is now + 120 s ({before}..{after}): {got}");
}
