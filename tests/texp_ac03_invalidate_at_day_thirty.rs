//! PRD-mcphost-external-unclaimed-tenant-expiry
//! AC3 (P0) — Given A idle 30 days with a `warned` event 10 days old, C idle 30
//! days whose first warn attempt is 3 days old, and D claimed at idle day 25,
//! When the sweep runs, Then only A ends `disabled_reason = 'unclaimed_expired'`,
//! torn down once, one notice sent, its `/u/` URL reading `tenant_disabled`
//! with the reason and `claim_url` and its key `tenant_key_invalid`; C and D
//! are untouched, and a keyless call from a new peer still mints.

use crate::common::texp::{self, DAY};
use crate::common::{EnvGuard, McpClient, extract_structured};
use mcphost::control::sweep_external_unclaimed;
use mcphost::email::FakeEmailClient;
use mcphost::kinds::KindRegistry;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn texp_ac03_only_the_warned_tenant_is_invalidated_and_the_front_door_stays_open() {
    let _env = EnvGuard::clear(texp::ENV).await;
    let removed = Arc::new(Mutex::new(Vec::new()));
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(texp::HookKind(removed.clone())));
    let email = Arc::new(FakeEmailClient::new());
    let server = texp::server(email.clone(), kinds).await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    texp::set_floor(&conn, now - 100 * DAY);

    let a_mint = texp::mint(&server).await;
    let c_mint = texp::mint(&server).await;
    let d_mint = texp::mint(&server).await;
    let a = texp::make_external(&conn, &a_mint.ns, now, 40, Some(30));
    let c = texp::make_external(&conn, &c_mint.ns, now, 40, Some(30));
    let d = texp::make_external(&conn, &d_mint.ns, now, 40, Some(30));
    let idle_since = now - 30 * DAY;
    texp::add_claim_attempt(&conn, a, "owner@example.com", now - 12 * DAY);
    texp::add_event(&conn, a, "warned", idle_since, now - 10 * DAY);
    texp::add_event(&conn, c, "warned", idle_since, now - 3 * DAY);
    texp::add_event(&conn, d, "warned", idle_since, now - 10 * DAY);
    conn.execute("UPDATE tenants SET owner_verified_at = ?2, claimed_unix = ?2 WHERE id = ?1", rusqlite::params![d, now - 5 * DAY]).unwrap();

    let out = sweep_external_unclaimed(&server.state, now).await.expect("sweep");
    assert_eq!(out["invalidated"], 1, "{out}");

    assert_eq!(texp::state(&conn, a), (true, Some("unclaimed_expired".to_string())));
    assert_eq!(texp::state(&conn, c), (false, None), "C was warned only 3 days ago");
    assert_eq!(texp::state(&conn, d), (false, None), "D claimed");
    assert_eq!(texp::count_stage(&conn, a, "invalidated"), 1);
    assert_eq!(texp::count_stage(&conn, c, "invalidated"), 0);
    assert_eq!(*removed.lock().unwrap(), vec![a], "torn down once, A only");

    let sends = email.sends();
    assert_eq!(sends.len(), 1, "one notice");
    assert_eq!(sends[0].to, "owner@example.com");
    let plain: String = conn.query_row("SELECT claim_token_plain FROM tenants WHERE id = ?1", [a], |r| r.get(0)).unwrap();
    let claim_url = mcphost::claim::claim_url(&server.state.public_url, &plain);
    assert!(sends[0].text_body.contains(&claim_url), "the notice carries the claim link: {}", sends[0].text_body);
    assert!(!sends[0].text_body.contains(&a_mint.key) && !sends[0].text_body.contains("/u/"), "no credential in the notice");

    // A's own URL names the reason and the claim link; its key reads invalid.
    let by_url = McpClient::new(&server.base_url).with_path(&a_mint.path);
    let url_err = by_url.tools_call("host.whoami", json!({})).await.expect_err("URL refused");
    assert_eq!(url_err.error_code.as_deref(), Some("tenant_disabled"), "{url_err:?}");
    assert_eq!(url_err.data["disabled_reason"], "unclaimed_expired");
    assert_eq!(url_err.data["claim_url"], claim_url);
    let key_err = McpClient::new(&server.base_url)
        .tools_call("host.whoami", json!({"tenant_key": a_mint.key}))
        .await
        .expect_err("key refused");
    assert_eq!(key_err.error_code.as_deref(), Some("tenant_key_invalid"), "{key_err:?}");
    assert!(key_err.data.get("claim_url").is_none(), "the key path never carries the link");

    // The front door stays open.
    let fresh = extract_structured(&McpClient::new(&server.base_url).tools_call("host.whoami", json!({})).await.expect("keyless mint"));
    assert_ne!(fresh["tenant"].as_str(), Some(a_mint.ns.as_str()));
    assert!(fresh["onboarding"]["url"].is_string(), "a new tenant was minted: {fresh}");
}
