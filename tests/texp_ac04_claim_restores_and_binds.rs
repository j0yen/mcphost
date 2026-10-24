//! PRD-mcphost-external-unclaimed-tenant-expiry
//! AC4 (P0) — Given A invalidated 5 days ago and E at idle day 30 whose claim
//! verification a test hook interleaves between the sweep's two phases, When
//! A's emailed URL is verified by its bound address and tried with another,
//! Then A is restored (`disabled = 0`, reason and `claim_bound_email` `NULL`,
//! `owner_verified_at` set, one `restored` event), the other address is
//! refused, and E ends claimed and enabled, never claimed-and-disabled.

use crate::common::texp::{self, DAY};
use crate::common::{EnvGuard, TestServer};
use mcphost::auth::hash_key;
use mcphost::control::sweep_external_unclaimed;
use mcphost::db::ClaimVerifyOutcome;
use mcphost::email::FakeEmailClient;
use mcphost::kinds::KindRegistry;
use std::sync::{Arc, Mutex};

fn code_from_email_body(body: &str) -> &str {
    let idx = body.find("/claim/verify/").expect("verify URL in email body");
    body[idx + "/claim/verify/".len()..].split_whitespace().next().expect("code token")
}

async fn invalidated_a(server: &TestServer, conn: &rusqlite::Connection, now: i64) -> (i64, String) {
    let m = texp::mint(server).await;
    let a = texp::make_external(conn, &m.ns, now, 60, Some(35));
    let token = "texp-claim-token-a";
    conn.execute(
        "UPDATE tenants SET disabled = 1, disabled_reason = 'unclaimed_expired', claim_token_hash = ?2, \
         claim_token_plain = ?3, claim_expires_at = ?4, claim_bound_email = 'owner@example.com' WHERE id = ?1",
        rusqlite::params![a, hash_key(token), token, now + 39 * DAY],
    )
    .unwrap();
    texp::add_claim_attempt(conn, a, "owner@example.com", now - 15 * DAY);
    texp::add_event(conn, a, "invalidated", now - 35 * DAY, now - 5 * DAY);
    (a, token.to_string())
}

#[tokio::test]
async fn texp_ac04_bound_address_restores_other_is_refused_and_a_claim_between_phases_wins() {
    let _env = EnvGuard::clear(texp::ENV).await;
    let email = Arc::new(FakeEmailClient::new());
    let server = texp::server(email.clone(), KindRegistry::with_builtin()).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let db_path = server.data_dir.0.join("mcphost.db");
    let conn = rusqlite::Connection::open(&db_path).expect("raw db");
    let now = mcphost::state::now_unix();
    texp::set_floor(&conn, now - 100 * DAY);
    let http = reqwest::Client::new();

    // A: the emailed URL, bound to owner@example.com.
    let (a, token) = invalidated_a(&server, &conn, now).await;
    let refused = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "other@example.com")])
        .send()
        .await
        .expect("POST other");
    assert_eq!(refused.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = refused.text().await.unwrap();
    assert!(body.contains("address this link was sent to"), "the claim form's own error line: {body}");
    assert_eq!(email.send_count(), 0, "no verify email went to the other address");
    let ok = http
        .post(format!("{}/claim/{token}", server.base_url))
        .form(&[("email", "Owner@Example.com")])
        .send()
        .await
        .expect("POST bound");
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
    let sends = email.sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].to, "Owner@Example.com");
    let code = code_from_email_body(&sends[0].text_body).to_string();
    let verified = http.get(format!("{}/claim/verify/{code}", server.base_url)).send().await.expect("verify");
    assert_eq!(verified.status(), reqwest::StatusCode::OK);
    assert_eq!(texp::state(&conn, a), (false, None), "A is enabled again");
    let (verified_at, bound): (Option<i64>, Option<String>) = conn
        .query_row("SELECT owner_verified_at, claim_bound_email FROM tenants WHERE id = ?1", [a], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert!(verified_at.is_some(), "owner_verified_at set");
    assert_eq!(bound, None, "binding cleared");
    assert_eq!(texp::count_stage(&conn, a, "restored"), 1);

    // E: a verification between the two phases is told to retry; F: another
    // claim path lands between the phases. Neither ends claimed and disabled.
    let e_mint = texp::mint(&server).await;
    let f_mint = texp::mint(&server).await;
    let e = texp::make_external(&conn, &e_mint.ns, now, 40, Some(30));
    let f = texp::make_external(&conn, &f_mint.ns, now, 40, Some(30));
    for id in [e, f] {
        texp::add_event(&conn, id, "warned", now - 30 * DAY, now - 10 * DAY);
    }
    conn.execute(
        "INSERT INTO claim_codes (tenant_id, code_hash, email, expires_unix, created_unix) \
         VALUES (?1, ?2, 'e@example.com', ?3, ?4)",
        rusqlite::params![e, hash_key("texp-code-e"), now + 1800, now - 60],
    )
    .unwrap();
    let outcomes: Arc<Mutex<Vec<ClaimVerifyOutcome>>> = Arc::new(Mutex::new(Vec::new()));
    let hook_db = server.state.db.clone();
    let hook_outcomes = outcomes.clone();
    let hook_path = db_path.clone();
    server.state.db.set_external_phase_hook_for_test(Arc::new(move |ids| {
        let db = hook_db.clone();
        let outcomes = hook_outcomes.clone();
        let path = hook_path.clone();
        Box::pin(async move {
            if ids.contains(&e) {
                let outcome = db.verify_claim_code(hash_key("texp-code-e")).await.expect("verify");
                outcomes.lock().unwrap().push(outcome);
            }
            if ids.contains(&f) {
                let raw = rusqlite::Connection::open(&path).expect("raw db in hook");
                raw.execute(
                    "UPDATE tenants SET owner_verified_at = ?2, claimed_unix = ?2, owner_email = 'f@example.com', \
                     owner_method = 'admin' WHERE id = ?1",
                    rusqlite::params![f, mcphost::state::now_unix()],
                )
                .expect("claim lands between the phases");
            }
        })
    }));

    let out = sweep_external_unclaimed(&server.state, now).await.expect("sweep");
    assert_eq!(*outcomes.lock().unwrap(), vec![ClaimVerifyOutcome::TryAgain], "mid-invalidation: retry, nothing consumed");
    assert_eq!(out["invalidated"], 1, "E was invalidated: {out}");
    assert_eq!(out["claimed_during_invalidation"], 1, "F was claimed meanwhile: {out}");
    assert_eq!(texp::state(&conn, e), (true, Some("unclaimed_expired".to_string())));
    // The retry after the sweep restores E in the same statement that claims it.
    match server.state.db.verify_claim_code(hash_key("texp-code-e")).await.expect("retry") {
        ClaimVerifyOutcome::Verified { tenant_id, .. } => assert_eq!(tenant_id, e),
        other => panic!("expected Verified, got {other:?}"),
    }
    assert_eq!(texp::state(&conn, e), (false, None), "E ends claimed and enabled");
    let e_claimed: Option<i64> = conn.query_row("SELECT owner_verified_at FROM tenants WHERE id = ?1", [e], |r| r.get(0)).unwrap();
    assert!(e_claimed.is_some());
    assert_eq!(texp::count_stage(&conn, e, "restored"), 1);
    assert_eq!(texp::state(&conn, f), (false, None), "F is claimed and enabled, never both claimed and disabled");
    assert_eq!(texp::count_stage(&conn, f, "invalidated"), 0);
}
