//! AC5 — Given a run that crashes after the POST but before the state
//! write (test injects the fault), When `emit-meter` reruns, Then the
//! resent batch carries identical `identifier` values and the ledger marks
//! it a replay.

use crate::common;
use common::{TestServer, record_ok_calls, signup_and_make_pro};
use mcphost::billing::FakeBillingClient;
use mcphost::metering;

#[tokio::test]
async fn crash_before_state_advance_replays_with_the_same_identifier() {
    let server = TestServer::start().await;
    let (_ns, _key, pro_id) =
        signup_and_make_pro(&server, "Crash Prone", "cus_metering_ac5").await;
    record_ok_calls(&server, pro_id, "some_tool", 10).await;

    let fake = FakeBillingClient::new(mcphost::state::now_unix());

    // Simulate a process crash landing after the POST/ledger writes but
    // before `meter_state.last_call_id` is advanced.
    let crashed = metering::run_once_without_state_advance(&server.state.db, &fake, "mcphost_tool_calls")
        .await
        .expect("the 'crashed' run itself must still complete its own steps");
    assert_eq!(crashed.groups.len(), 1);
    assert_eq!(crashed.groups[0].mode, "sent");
    let identifier_before = fake.all_meter_events()[0].identifier.clone();

    // meter_state was deliberately left unadvanced.
    let (last_call_id, _) = server.state.db.get_meter_state().await.unwrap();
    assert_eq!(last_call_id, 0, "the crash must not have advanced state");

    // The real rerun: recomputes the identical (still unadvanced) span.
    let rerun = metering::run_once(&server.state.db, &fake, "mcphost_tool_calls")
        .await
        .expect("rerun must succeed");
    assert_eq!(rerun.groups.len(), 1);
    assert_eq!(
        rerun.groups[0].mode, "replay",
        "the ledger must mark the resend as a replay"
    );

    let events = fake.all_meter_events();
    assert_eq!(events.len(), 2, "both the crashed attempt and the rerun POSTed");
    assert_eq!(
        events[0].identifier, events[1].identifier,
        "the resent batch must carry an identical identifier"
    );
    assert_eq!(events[1].identifier, identifier_before);

    // The rerun completes what the crashed run could not: state advances.
    let (last_call_id_after, _) = server.state.db.get_meter_state().await.unwrap();
    assert_eq!(last_call_id_after, rerun.groups[0].last_call_id);

    // No call was ever double-counted: exactly 10 calls covered per
    // attempt, and only one call span exists in the calls table.
    assert_eq!(crashed.calls_covered, 10);
    assert_eq!(rerun.calls_covered, 10);
}
