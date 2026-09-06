//! AC7 — Given 120 pending pro tenants' batches, When one run executes,
//! Then no request carries more than 100 events and all pending spans are
//! covered across requests.

mod common;
use common::{TestServer, record_ok_calls, signup_and_make_pro};
use mcphost::billing::FakeBillingClient;
use mcphost::metering;

#[tokio::test]
async fn one_hundred_twenty_tenants_split_across_two_requests() {
    // 120 signups from this test's single source IP would otherwise trip
    // the default signup rate limit (5/hour) long before reaching 120.
    let server = TestServer::start_with_signup_rate_limit(200).await;

    let mut tenant_ids = Vec::with_capacity(120);
    for i in 0..120 {
        let (_ns, _key, tenant_id) = signup_and_make_pro(
            &server,
            &format!("Tenant {i}"),
            &format!("cus_metering_ac7_{i}"),
        )
        .await;
        record_ok_calls(&server, tenant_id, "some_tool", 1).await;
        tenant_ids.push(tenant_id);
    }

    let fake = FakeBillingClient::new(mcphost::state::now_unix());
    let outcome = metering::run_once(&server.state.db, &fake, "mcphost_tool_calls")
        .await
        .expect("emit-meter run");

    assert_eq!(outcome.groups.len(), 120, "every tenant's span must be covered");
    assert_eq!(outcome.events_sent, 120);
    assert_eq!(outcome.requests_sent, 2, "120 events at 100/request is 2 requests");

    let batches = fake.meter_batches.lock().unwrap();
    assert_eq!(batches.len(), 2);
    for batch in batches.iter() {
        assert!(
            batch.len() <= metering::MAX_EVENTS_PER_REQUEST,
            "no request may carry more than {} events, got {}",
            metering::MAX_EVENTS_PER_REQUEST,
            batch.len()
        );
    }
    assert_eq!(batches[0].len() + batches[1].len(), 120);

    // Every tenant is covered exactly once.
    let ledgered_tenants: std::collections::HashSet<i64> =
        outcome.groups.iter().map(|g| g.tenant_id).collect();
    assert_eq!(ledgered_tenants.len(), 120);
    for id in &tenant_ids {
        assert!(ledgered_tenants.contains(id));
    }
}
