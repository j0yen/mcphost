//! AC6 — Given the fake returns 500, When `emit-meter` runs, Then it
//! exits non-zero (an `Err` here, since this test drives the library
//! function directly rather than the CLI binary), `meter_state` is
//! unmoved, and no ledger row is written.

mod common;
use common::{TestServer, record_ok_calls, signup_and_make_pro};
use mcphost::billing::FakeBillingClient;
use mcphost::metering;

#[tokio::test]
async fn a_failed_post_leaves_state_and_ledger_untouched() {
    let server = TestServer::start().await;
    let (_ns, _key, pro_id) =
        signup_and_make_pro(&server, "Failure Prone", "cus_metering_ac6").await;
    record_ok_calls(&server, pro_id, "some_tool", 5).await;

    let fake = FakeBillingClient::new(mcphost::state::now_unix());
    fake.set_fail_meter_events(true);

    let result = metering::run_once(&server.state.db, &fake, "mcphost_tool_calls").await;
    assert!(result.is_err(), "a 500 from the fake must propagate as an Err");

    let (last_call_id, updated_at) = server.state.db.get_meter_state().await.unwrap();
    assert_eq!(last_call_id, 0, "meter_state must be unmoved");
    assert_eq!(updated_at, None);

    let ledger_rows = server.state.db.count_meter_events(Some(pro_id)).await.unwrap();
    assert_eq!(ledger_rows, 0, "no ledger row must be written on failure");

    // The pending span is still fully visible for a future retry.
    let lag = server.state.db.meter_lag(0).await.unwrap();
    assert_eq!(lag, 5);
}
