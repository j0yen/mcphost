//! PRD-mcphost-external-unclaimed-tenant-expiry
//! AC2 (P0) — Given subject A idle 20 days with one claim attempt and subject
//! B idle 20 days with none, When the sweep runs twice on day 20 and once on
//! day 21, Then one `FakeEmailClient` send goes to A's newest
//! `claim_codes.email`, naming 10 days left and A's claim URL, without A's key
//! or `/u/` secret, A's `claim_expires_at` is `idle_since + 44 d`, A has one
//! `warned` and B two `warn_no_address` events, and runs two and three send
//! nothing.

use crate::common::texp::{self, DAY};
use crate::common::{EnvGuard, TestServer, signup};
use mcphost::control::sweep_external_unclaimed;
use mcphost::email::FakeEmailClient;
use std::sync::Arc;

#[tokio::test]
async fn texp_ac02_one_warning_to_the_newest_claim_address_and_one_event_per_day() {
    let _env = EnvGuard::clear(texp::ENV).await;
    let email = Arc::new(FakeEmailClient::new());
    let server = TestServer::start_with_email(email.clone()).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    texp::set_floor(&conn, now - 100 * DAY);

    let (a_ns, a_key) = signup(&server.base_url, "Texp warn A").await;
    let (b_ns, _b_key) = signup(&server.base_url, "Texp warn B").await;
    let a = texp::make_external(&conn, &a_ns, now, 30, Some(20));
    let b = texp::make_external(&conn, &b_ns, now, 30, Some(20));
    texp::add_claim_attempt(&conn, a, "older@example.com", now - 5 * DAY);
    texp::add_claim_attempt(&conn, a, "owner@example.com", now - 2 * DAY);
    let idle_since = now - 20 * DAY;

    let run1 = sweep_external_unclaimed(&server.state, now).await.expect("sweep 1");
    let run2 = sweep_external_unclaimed(&server.state, now + 60).await.expect("sweep 2");
    let run3 = sweep_external_unclaimed(&server.state, now + DAY + 60).await.expect("sweep 3");

    assert_eq!((run1["warned"].as_i64(), run1["warn_no_address"].as_i64()), (Some(1), Some(1)), "{run1}");
    assert_eq!((run2["warned"].as_i64(), run2["warn_no_address"].as_i64()), (Some(0), Some(0)), "{run2}");
    assert_eq!((run3["warned"].as_i64(), run3["warn_no_address"].as_i64()), (Some(0), Some(1)), "{run3}");

    let sends = email.sends();
    assert_eq!(sends.len(), 1, "exactly one email over three runs");
    let msg = &sends[0];
    assert_eq!(msg.to, "owner@example.com", "the newest claim attempt's address");
    let (plain, expires): (String, i64) = conn
        .query_row("SELECT claim_token_plain, claim_expires_at FROM tenants WHERE id = ?1", [a], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("claim token");
    let url = mcphost::claim::claim_url(&server.state.public_url, &plain);
    let text = format!("{}\n{}", msg.subject, msg.text_body);
    assert!(text.contains(&url), "the body carries A's claim URL {url}: {text}");
    assert!(text.contains("10 days"), "names the days left: {text}");
    assert!(!text.contains(&a_key), "never the tenant key");
    assert!(!text.contains("/u/"), "never a /u/ secret URL");
    assert_eq!(expires, idle_since + 44 * DAY, "the link resolves through the purge");

    assert_eq!(texp::count_stage(&conn, a, "warned"), 1);
    assert_eq!(texp::count_stage(&conn, a, "warn_no_address"), 0);
    assert_eq!(texp::count_stage(&conn, b, "warn_no_address"), 2);
    assert_eq!(texp::count_stage(&conn, b, "warned"), 0);
    assert_eq!(texp::state(&conn, a), (false, None), "a warning disables nothing");
}
