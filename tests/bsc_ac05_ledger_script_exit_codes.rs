//! PRD-mcphost-billing-selfcheck-no-ledger-row AC5 -- `scripts/live/
//! billing-selfcheck-ledger.sh <base-url>` exits 0 on a ledger with no
//! self-check row, 1 with one, 3 when billing is failing; the admin key
//! (env `MCPHOST_ADMIN_KEY`) is never printed.

use crate::common::{ADMIN_KEY, TestServer};
use mcphost::db::BillingEventInsert;

async fn run_script(base_url: String) -> (String, i32) {
    tokio::task::spawn_blocking(move || {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/live/billing-selfcheck-ledger.sh");
        let out = std::process::Command::new("bash")
            .arg(script)
            .arg(base_url)
            .env("MCPHOST_ADMIN_KEY", ADMIN_KEY)
            .output()
            .expect("run script");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (text, out.status.code().unwrap_or(-1))
    })
    .await
    .expect("join")
}

#[tokio::test]
async fn bsc_ac05_script_exits_0_then_1_then_3() {
    let server = TestServer::start().await;
    mcphost::statusfeed::tick_once(&server.state).await.expect("tick");

    let (out, rc) = run_script(server.base_url.clone()).await;
    assert_eq!(rc, 0, "{out}");
    assert!(!out.contains(ADMIN_KEY));

    server
        .state
        .db
        .insert_billing_event(BillingEventInsert {
            event_id: "evt_status_self_check_1".to_string(),
            event_type: "status.self_check".to_string(),
            tenant_id: None,
            plan: None,
            amount_cents: None,
            currency: None,
            mode: "test".to_string(),
            payload_sha256: "x".to_string(),
            source: None,
            session_id: None,
        })
        .await
        .expect("insert");
    let (out, rc) = run_script(server.base_url.clone()).await;
    assert_eq!(rc, 1, "{out}");
    assert!(!out.contains(ADMIN_KEY));

    server.state.status_probe_override.set("billing", false);
    mcphost::statusfeed::tick_once(&server.state).await.expect("tick");
    let (out, rc) = run_script(server.base_url.clone()).await;
    assert_eq!(rc, 3, "{out}");
    assert!(!out.contains(ADMIN_KEY));
}
