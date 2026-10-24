//! PRD-mcphost-synthetic-implicit-tenant-expiry
//! AC6 (P1) — Given `host.doctor` called by a subject tenant, a claimed
//! tenant, an external tenant, and the subject again with the window set to 0,
//! When each reads the response, Then `tenant_lifecycle` is `{subject: true,
//! idle_days, expires_at_unix}` with `expires_at_unix` equal to the call's
//! `now + window` for the subject, and `{subject: false, expires_at_unix:
//! null}` for the other three, computed from the `CLASS` and `FLOOR` terms.

use crate::common::tenx::{self, DAY};
use crate::common::{EnvGuard, McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn lifecycle(server: &TestServer, key: &str) -> (Value, i64, i64) {
    let before = mcphost::state::now_unix();
    let out = extract_structured(
        &McpClient::with_bearer(&server.base_url, key).tools_call("host.doctor", json!({})).await.expect("host.doctor"),
    );
    (out["tenant_lifecycle"].clone(), before, mcphost::state::now_unix())
}

#[tokio::test]
async fn tenx_ac06_doctor_says_whether_and_when_a_tenant_expires() {
    let _env = EnvGuard::clear(tenx::ENV).await;
    let server = TestServer::start_with_signup_rate_limit(1000).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    tenx::set_floor(&conn, now - 10 * DAY);

    let mut keys = Vec::new();
    for (label, tweak) in [
        ("subject", ""),
        ("claimed", ", claimed_unix = created_unix"),
        ("external", ", origin = 'external'"),
    ] {
        let (ns, key) = signup(&server.base_url, &format!("Tenx {label}")).await;
        conn.execute(
            &format!(
                "UPDATE tenants SET signup_source = 'implicit', origin = 'synthetic', synthetic = 'synthorg:test', \
                 claimed_unix = NULL, owner_verified_at = NULL, paid_unix = NULL, billing_ref = NULL, \
                 created_unix = ?2, last_seen_unix = ?2 {tweak} WHERE namespace = ?1"
            ),
            rusqlite::params![ns, now - 3 * DAY],
        )
        .expect("shape the tenant");
        keys.push(key);
    }

    let (subject, t0, t1) = lifecycle(&server, &keys[0]).await;
    assert_eq!(subject["subject"], true, "{subject}");
    assert_eq!(subject["idle_days"], 1);
    let at = subject["expires_at_unix"].as_i64().expect("expires_at_unix");
    assert!((t0 + DAY..=t1 + DAY).contains(&at), "expires_at_unix {at} is the call's now + 1 day ({t0}..{t1})");

    for key in &keys[1..] {
        let (view, ..) = lifecycle(&server, key).await;
        assert_eq!(view, json!({"subject": false, "expires_at_unix": null}));
    }

    server.state.db.set_retention_days_for_test(tenx::POLICY.to_string(), 0).await.expect("window 0");
    let (off, ..) = lifecycle(&server, &keys[0]).await;
    assert_eq!(off, json!({"subject": false, "expires_at_unix": null}), "window 0 reports subject: false");
}
