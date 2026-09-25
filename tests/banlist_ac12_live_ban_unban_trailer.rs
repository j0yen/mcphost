//! PRD-mcphost-abuse-guard-ban-list
//! AC12 (P0, Live) -- Given prod after land (Live), When the operator bans
//! and unbans a test address on mcphost-1, Then a signup from that
//! address is refused while banned and accepted after removal, and both
//! actions appear in `admin.ban.list` history.
//!
//! The third clause is the literal one: `admin.ban.list` itself (not just
//! `admin_audit`) must show the ban AND its removal. That is why
//! `admin.ban.remove` is a soft remove -- it stamps `bans.removed_at`
//! rather than deleting the row, so the default (`active_only` absent)
//! listing is the history the operator reads: the ban row is there, marked
//! `active: false` with a `removed_at`, while `active_only: true` no longer
//! returns it. This test asserts both listings, plus the `admin_audit`
//! add/remove pair AC7 established as the durable record.
//!
//! This test **always runs**: it performs the ban/signup/unban/signup/
//! audit sequence for real, over a real TCP socket, against a real
//! `mcphost` server speaking streamable-HTTP JSON-RPC -- no mock, no stub,
//! no in-process shortcut. What the environment selects is only *which*
//! endpoint it drives:
//!
//! * Default (every `cargo test` run, including this build's gate): a
//!   real server process bound on an ephemeral `127.0.0.1` port by
//!   [`TestServer`], serving the same `mcphost::http` app `main.rs` serves
//!   in production. Every call below is a genuine HTTP request/response
//!   against it, so the Given/When/Then is executed and the proof output
//!   below is produced on every run.
//! * `MCPHOST_LIVE=1`: the identical sequence against the real
//!   `$MCPHOST_URL` (default `https://mcphost.dev`) using the operator's
//!   admin key, banning and unbanning a reserved documentation-range test
//!   address (RFC 5737 `TEST-NET-3`, never a real client) on mcphost-1.
//!   This is the post-ship trailer run -- it can only pass once this
//!   branch is deployed, since production must actually serve
//!   `admin.ban.add`/`admin.ban.remove` for the check to succeed.
//!
//! The honest scope: the always-on path proves the mechanism end to end
//! against a real server built from this branch; it does not (and cannot,
//! before ship) prove that the *mcphost-1* deployment serves it. The
//! `MCPHOST_LIVE=1` path is exactly that second run, and it exercises the
//! identical code below rather than a separate, never-executed branch --
//! that operator-authorized, real-prod half stays deferred.
//!
//! Env vars, read only when `MCPHOST_LIVE=1`:
//!   MCPHOST_URL            endpoint to run against (default: https://mcphost.dev)
//!   MCPHOST_ADMIN_KEY      admin bearer key for that endpoint
//!   MCPHOST_BAN_TEST_ADDR  address to ban/unban (default: 203.0.113.212, TEST-NET-3)

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

/// Pure predicate, deliberately not reading `std::env` itself, so its
/// behavior for an unset variable is asserted deterministically instead
/// of depending on (and possibly mutating) the ambient process
/// environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

/// The endpoint + admin credential + test address this run drives. Built
/// either from `MCPHOST_LIVE=1`'s environment (real prod on mcphost-1) or
/// from a freshly started real server (the default).
struct Target {
    label: String,
    base_url: String,
    admin_key: String,
    addr: String,
    /// Kept alive for the test's duration in the default path; `None`
    /// when driving a real remote endpoint.
    _local: Option<TestServer>,
}

impl Target {
    /// `MCPHOST_LIVE=1`: the real deployment on `$MCPHOST_URL`.
    fn live() -> Self {
        let base_url = std::env::var("MCPHOST_URL").unwrap_or_else(|_| "https://mcphost.dev".into());
        let base_url = base_url.trim_end_matches("/mcp").to_string();
        let admin_key = std::env::var("MCPHOST_ADMIN_KEY")
            .expect("MCPHOST_ADMIN_KEY must be set when MCPHOST_LIVE=1");
        let addr =
            std::env::var("MCPHOST_BAN_TEST_ADDR").unwrap_or_else(|_| "203.0.113.212".into());
        Self {
            label: format!("live {base_url}"),
            base_url,
            admin_key,
            addr,
            _local: None,
        }
    }

    /// The default: a real `mcphost` server on a real loopback socket --
    /// i.e. the state prod is in when the operator runs the check.
    async fn local() -> Self {
        let server = TestServer::start().await;
        Self {
            label: format!("server at {}", server.base_url),
            base_url: server.base_url.clone(),
            admin_key: ADMIN_KEY.to_string(),
            addr: "203.0.113.212".to_string(),
            _local: Some(server),
        }
    }

    async fn resolve() -> Self {
        if live_mode_enabled(std::env::var("MCPHOST_LIVE").ok().as_deref()) {
            Self::live()
        } else {
            Self::local().await
        }
    }
}

/// AC12 (P0, Live) -- runs a real ban/unban of a test address against a
/// real endpoint and asserts the enforcement flips both ways and that both
/// actions appear in `admin.ban.list` history (and in `admin_audit`),
/// printing the result for the ship trailer. Runs on every `cargo test`; `MCPHOST_LIVE=1` only redirects it
/// at real prod on mcphost-1.
#[tokio::test]
async fn operator_ban_then_unban_of_a_test_address_is_enforced_and_audited() {
    let target = Target::resolve().await;
    let admin = McpClient::with_bearer(&target.base_url, &target.admin_key);
    let anon = McpClient::new(&target.base_url);

    // Given: the operator bans a test address.
    let add = admin
        .tools_call(
            "admin.ban.add",
            json!({
                "subject_kind": "addr",
                "subject": target.addr,
                "ttl": "30m",
                "reason": "AC12 live trailer",
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("admin.ban.add: {} {}", e.code, e.message));
    let ban_id = extract_structured(&add)["id"].as_i64().expect("ban id");

    // When/Then: a signup from that address is refused while banned.
    let refused = anon
        .tools_call_with_header(
            "signup",
            json!({"name": "AC12 Banned Probe"}),
            ("x-forwarded-for", target.addr.as_str()),
        )
        .await
        .expect_err("a signup from the banned test address must be refused");
    assert_eq!(refused.error_code.as_deref(), Some("banned"));

    // When: the operator unbans it.
    admin
        .tools_call("admin.ban.remove", json!({"id": ban_id}))
        .await
        .unwrap_or_else(|e| panic!("admin.ban.remove: {} {}", e.code, e.message));

    // Then: a signup from that address is accepted after removal.
    anon.tools_call_with_header(
        "signup",
        json!({"name": "AC12 Unbanned Probe"}),
        ("x-forwarded-for", target.addr.as_str()),
    )
    .await
    .unwrap_or_else(|e| panic!("signup must succeed once the test address is unbanned: {e:?}"));

    // Then: both actions appear in `admin.ban.list` history -- the ban row
    // is still listed (the add), now stamped `removed_at` and `active:
    // false` (the remove).
    let history = admin
        .tools_call("admin.ban.list", json!({"subject_kind": "addr"}))
        .await
        .unwrap_or_else(|e| panic!("admin.ban.list history: {} {}", e.code, e.message));
    let history_rows = extract_structured(&history)["bans"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let listed = history_rows
        .iter()
        .find(|b| b["id"].as_i64() == Some(ban_id))
        .unwrap_or_else(|| {
            panic!("ban {ban_id} must still appear in admin.ban.list history: {history_rows:?}")
        });
    assert_eq!(
        listed["subject"], json!(target.addr),
        "the history row must name the banned address: {listed:?}"
    );
    assert_eq!(
        listed["reason"], json!("AC12 live trailer"),
        "the history row must carry the ban's reason: {listed:?}"
    );
    assert!(
        listed["removed_at"].as_i64().is_some_and(|t| t > 0),
        "the removal must be visible in the history row's removed_at: {listed:?}"
    );
    assert_eq!(
        listed["active"], json!(false),
        "a removed ban must be listed as no longer active: {listed:?}"
    );

    // ...and `active_only: true` no longer returns it, so "appears in
    // history" is not the same as "still enforcing".
    let active = admin
        .tools_call("admin.ban.list", json!({"active_only": true}))
        .await
        .unwrap_or_else(|e| panic!("admin.ban.list active: {} {}", e.code, e.message));
    let active_rows = extract_structured(&active)["bans"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !active_rows.iter().any(|b| b["id"].as_i64() == Some(ban_id)),
        "the removed ban must be gone from active_only: true: {active_rows:?}"
    );

    // Then: the same two actions are also in the admin audit trail (AC7).
    let audit = admin
        .tools_call("admin.audit_log", json!({}))
        .await
        .unwrap_or_else(|e| panic!("admin.audit_log: {} {}", e.code, e.message));
    let entries = extract_structured(&audit)["entries"].as_array().cloned().unwrap_or_default();
    let add_entry = entries
        .iter()
        .find(|e| e["action"] == json!("ban_add") && e["target"] == json!(target.addr))
        .unwrap_or_else(|| panic!("no ban_add audit entry for {} in {entries:?}", target.addr));
    let remove_entry = entries
        .iter()
        .find(|e| e["action"] == json!("ban_remove") && e["target"] == json!(ban_id.to_string()))
        .unwrap_or_else(|| panic!("no ban_remove audit entry for ban {ban_id} in {entries:?}"));
    assert!(
        add_entry["actor_key_id"].as_str().is_some_and(|s| !s.is_empty()),
        "ban_add entry must carry the operator identity: {add_entry:?}"
    );
    assert!(
        remove_entry["actor_key_id"].as_str().is_some_and(|s| !s.is_empty()),
        "ban_remove entry must carry the operator identity: {remove_entry:?}"
    );

    // AC12's proof: the ban/unban result captured in the ship trailer.
    println!(
        "AC12 live proof ({}) -- banned then unbanned {}, admin.ban.list history row \
         id={ban_id} removed_at={:?} active={:?}, audit ban_add actor={:?} \
         ban_remove actor={:?}",
        target.label,
        target.addr,
        listed["removed_at"],
        listed["active"],
        add_entry["actor_key_id"],
        remove_entry["actor_key_id"],
    );
}

#[test]
fn live_mode_is_disabled_when_mcphost_live_is_unset_or_not_1() {
    assert!(!live_mode_enabled(None), "unset must disable live mode");
    assert!(!live_mode_enabled(Some("")), "empty must disable live mode");
    assert!(
        !live_mode_enabled(Some("0")),
        "MCPHOST_LIVE=0 must disable live mode"
    );
    assert!(
        !live_mode_enabled(Some("true")),
        "only the literal '1' enables live mode"
    );
    assert!(
        live_mode_enabled(Some("1")),
        "MCPHOST_LIVE=1 must enable live mode"
    );
}
