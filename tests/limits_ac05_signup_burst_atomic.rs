//! AC5 (PRD-mcphost-call-limits-honest) — Given a signup cap of 5, When 10
//! concurrent signups arrive from one source, Then exactly 5 succeed and 5
//! are `rate_limited`.
//!
//! Before this PRD, `control::signup` read `Db::signup_count_since` and
//! only later wrote `Db::record_signup_event_attributed` -- two separate
//! `with_conn` calls, each holding the connection mutex only for its own
//! query, leaving a window where a concurrent burst could all read
//! `recent < limit` before any of them had recorded its own event
//! (`Db::try_admit_signup` closes this by doing both in one `with_conn`
//! call / one SQL statement). This test fires the burst concurrently
//! (`tokio::spawn`, not a sequential loop like `ac09_signup_rate_limit.rs`)
//! specifically to exercise that race.

mod common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn ten_concurrent_signups_at_cap_five_admit_exactly_five() {
    let server = TestServer::start_with_signup_rate_limit(5).await;

    let mut handles = Vec::with_capacity(10);
    for i in 0..10 {
        let base_url = server.base_url.clone();
        handles.push(tokio::spawn(async move {
            let client = McpClient::new(&base_url);
            client
                .tools_call("signup", json!({"name": format!("Burst Agent {i}")}))
                .await
        }));
    }

    let mut ok = 0;
    let mut rate_limited = 0;
    for handle in handles {
        match handle.await.expect("task must not panic") {
            Ok(_) => ok += 1,
            Err(e) if e.error_code.as_deref() == Some("rate_limited") => rate_limited += 1,
            Err(e) => panic!("unexpected error: {} {}", e.code, e.message),
        }
    }

    assert_eq!(ok, 5, "exactly the cap must be admitted from a concurrent burst");
    assert_eq!(rate_limited, 5, "the rest must be rate_limited, not silently overshoot");

    let tenants = server.state.db.list_tenants().await.unwrap();
    assert_eq!(
        tenants.len(),
        5,
        "exactly 5 tenants must exist -- an admitted signup that later failed \
         tenant creation would undercount this"
    );
}
