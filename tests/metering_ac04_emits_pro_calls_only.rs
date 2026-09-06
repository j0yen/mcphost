//! AC4 — Given 250 ok pro calls, 40 ok free calls, and 10 failed pro calls
//! above the high-water mark, When `billing emit-meter` runs against the
//! fake, Then the fake receives events summing to 250 for the pro tenant
//! only, `meter_state.last_call_id` is the newest pro ok call id, and one
//! ledger row records the batch span.

mod common;
use common::{TestServer, record_ok_calls, signup, signup_and_make_pro};
use mcphost::billing::FakeBillingClient;
use mcphost::metering;

#[tokio::test]
async fn only_pro_ok_calls_are_emitted() {
    let server = TestServer::start().await;
    let (_ns, _key, pro_id) =
        signup_and_make_pro(&server, "Pro Caller", "cus_metering_ac4").await;
    let (_free_ns, _free_key) = signup(&server.base_url, "Free Caller").await;
    let free_tenant = server
        .state
        .db
        .find_tenant_by_namespace(_free_ns.clone())
        .await
        .unwrap()
        .unwrap();

    // 250 ok pro calls -- the only ones that should be emitted.
    record_ok_calls(&server, pro_id, "some_tool", 250).await;
    // 40 ok free calls -- excluded (plan != 'pro').
    record_ok_calls(&server, free_tenant.id, "some_tool", 40).await;
    // 10 failed pro calls -- excluded (ok = 0).
    for _ in 0..10 {
        server
            .state
            .db
            .record_call(pro_id, "some_tool".to_string(), 1, false, Some("boom".to_string()), None, None)
            .await
            .expect("record failed call");
    }

    let fake = FakeBillingClient::new(mcphost::state::now_unix());
    let outcome = metering::run_once(&server.state.db, &fake, "mcphost_tool_calls")
        .await
        .expect("emit-meter run");

    assert_eq!(outcome.calls_covered, 250);
    assert_eq!(outcome.requests_sent, 1);
    assert_eq!(outcome.groups.len(), 1);
    assert_eq!(outcome.groups[0].tenant_id, pro_id);
    assert_eq!(outcome.groups[0].count, 250);
    assert_eq!(outcome.groups[0].mode, "sent");

    let events = fake.all_meter_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].value, 250);
    assert_eq!(events[0].stripe_customer_id, "cus_metering_ac4");

    let (last_call_id, _) = server.state.db.get_meter_state().await.unwrap();
    assert_eq!(
        last_call_id,
        outcome.groups[0].last_call_id,
        "meter_state.last_call_id must be the newest pro ok call id"
    );

    // Exactly one ledger row records the batch span.
    let ledger_rows = server.state.db.count_meter_events(Some(pro_id)).await.unwrap();
    assert_eq!(ledger_rows, 1);

    let lag_after = server.state.db.meter_lag(last_call_id).await.unwrap();
    assert_eq!(lag_after, 0, "everything pending must now be covered");
}
