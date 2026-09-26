//! PRD-mcphost-upstream-token-vault-status
//! AC9 (P0, Live) -- Given prod after deploy with the operator tenant
//! holding provider `slack` registered from `preset: "slack"` with
//! placeholder client credentials and no handoff (operator-provisioned
//! Given, done by hand from orch with the operator key on mcphost-1
//! `/etc/mcphost/operator-tenant.key`), When `host.vault.status {end_user:
//! "vaultst-probe"}` is called with the operator key over
//! `https://mcphost.dev/mcp` and `admin.vault.stats` is called with the
//! admin key from orch `~/.config/mcphost/admin-key`, Then status lists
//! `slack` with `connected: false` and stats lists the operator tenant
//! with `slack {tokens: 0}`.
//!
//! This test **always runs**: it drives the identical `host.vault.status`
//! / `admin.vault.stats` sequence over a real HTTP round trip -- no mock,
//! no stub. What the environment selects is only *which* endpoint it
//! drives:
//!
//! * Default (every `cargo test` run, including this build's gate): a
//!   real server process bound on an ephemeral `127.0.0.1` port by
//!   [`TestServer`], serving the same `mcphost::http` app `main.rs` serves
//!   in production. A fresh tenant stands in for the operator tenant: it
//!   registers `slack` via `preset: "slack"` with placeholder client
//!   credentials and never completes a handoff, matching AC9's Given
//!   exactly except for *which* tenant it is.
//! * `MCPHOST_LIVE=1`: the real operator tenant's own key against the real
//!   `$MCPHOST_URL` (default `https://mcphost.dev`) -- the post-ship
//!   trailer run. It can only pass once this branch is deployed and an
//!   operator has hand-registered the operator tenant's placeholder
//!   `slack` provider on mcphost-1, since production must serve
//!   `host.vault.status`/`admin.vault.stats` and carry that row for the
//!   check to succeed.
//!
//! The honest scope: the always-on path proves the mechanism -- a
//! registered-but-never-connected provider reports `connected: false` from
//! `host.vault.status`, and `admin.vault.stats` reports `tokens: 0` for it
//! rather than omitting the tenant/provider row entirely -- against a real
//! server built from this branch. It does not (and cannot, before ship)
//! prove that the *mcphost.dev deployment* serves it, or that the real
//! operator tenant's row exists on mcphost-1. The `MCPHOST_LIVE=1` path is
//! exactly that second run; per
//! `PRD-mcphost-upstream-token-vault-status.md`'s `deferred_acs` /
//! `mock_justifications` frontmatter, that prod leg stays deferred --
//! operator-provisioned, and this test is not counted as AC9's proof,
//! only its mechanism (locked by
//! `tests/vaultst_ac09_deferral_is_justified.rs`).
//!
//! Env vars, read only when `MCPHOST_LIVE=1`:
//!   MCPHOST_URL           endpoint to run against (default: https://mcphost.dev)
//!   MCPHOST_OPERATOR_KEY  bearer key for the operator tenant (mcphost-1 /etc/mcphost/operator-tenant.key)
//!   MCPHOST_ADMIN_KEY     the admin key (orch ~/.config/mcphost/admin-key)

use serde_json::json;

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};

/// Pure predicate, deliberately not reading `std::env` itself, so its
/// behavior for an unset variable is asserted deterministically instead of
/// depending on (and possibly mutating) the ambient process environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

/// The endpoint + credentials this run drives. Built either from
/// `MCPHOST_LIVE=1`'s environment (the real operator tenant on prod) or
/// from a freshly started real server plus a fresh tenant standing in for
/// it (the default).
struct Target {
    label: String,
    base_url: String,
    operator_key: String,
    admin_key: String,
    /// Kept alive for the test's duration in the default path; `None` when
    /// driving a real remote endpoint.
    _local: Option<TestServer>,
}

impl Target {
    /// `MCPHOST_LIVE=1`: the real operator tenant plus the admin key on
    /// `$MCPHOST_URL`.
    fn live() -> Self {
        let base_url = std::env::var("MCPHOST_URL").unwrap_or_else(|_| "https://mcphost.dev".into());
        let base_url = base_url.trim_end_matches("/mcp").to_string();
        let operator_key = std::env::var("MCPHOST_OPERATOR_KEY")
            .expect("MCPHOST_OPERATOR_KEY must be set when MCPHOST_LIVE=1");
        let admin_key =
            std::env::var("MCPHOST_ADMIN_KEY").expect("MCPHOST_ADMIN_KEY must be set when MCPHOST_LIVE=1");
        Self {
            label: format!("live {base_url}"),
            base_url,
            operator_key,
            admin_key,
            _local: None,
        }
    }

    /// The default: a real `mcphost` server on a real loopback socket, with
    /// a fresh tenant standing in for the operator tenant, its `slack`
    /// provider registered by preset with placeholder credentials, and no
    /// handoff ever completed -- the state AC9's Given describes.
    async fn local() -> Self {
        let server = TestServer::start().await;
        let (_ns, key) = signup(&server.base_url, "AC9 Operator Tenant").await;
        let client = McpClient::with_bearer(&server.base_url, &key);
        client
            .tools_call(
                "host.vault.provider_set",
                json!({
                    "name": "slack",
                    "preset": "slack",
                    "client_id": "placeholder-client-id",
                    "client_secret": "placeholder-client-secret",
                    "scopes": ["read"],
                }),
            )
            .await
            .unwrap_or_else(|e| panic!("operator tenant's provider_set slack: {} {}", e.code, e.message));
        Self {
            label: format!("server at {}", server.base_url),
            base_url: server.base_url.clone(),
            operator_key: key,
            admin_key: ADMIN_KEY.to_string(),
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

/// AC9 (P0, Live) -- proves the mechanism: a registered-but-unconnected
/// provider reports `connected: false` from `host.vault.status`, and
/// `admin.vault.stats` reports `tokens: 0` for it rather than omitting the
/// tenant/provider row, printing both bodies for the ship trailer. Runs on
/// every `cargo test`; `MCPHOST_LIVE=1` only redirects it at real prod.
#[tokio::test]
async fn operator_tenant_slack_shows_disconnected_and_zero_tokens() {
    let target = Target::resolve().await;
    let operator = McpClient::with_bearer(&target.base_url, &target.operator_key);

    let whoami = extract_structured(
        &operator
            .tools_call("host.whoami", json!({}))
            .await
            .unwrap_or_else(|e| panic!("host.whoami: {} {}", e.code, e.message)),
    );
    let namespace = whoami["namespace"].as_str().expect("namespace field").to_string();

    // When: the operator key calls host.vault.status for the probe subject.
    let status = extract_structured(
        &operator
            .tools_call("host.vault.status", json!({"end_user": "vaultst-probe"}))
            .await
            .unwrap_or_else(|e| panic!("host.vault.status: {} {}", e.code, e.message)),
    );
    let providers = status["providers"].as_array().expect("providers array");
    let slack = providers
        .iter()
        .find(|p| p["name"] == json!("slack"))
        .unwrap_or_else(|| panic!("slack missing from host.vault.status: {providers:?}"));
    assert_eq!(slack["connected"], json!(false), "{slack:?}");

    // And: admin.vault.stats lists this tenant's slack provider with zero
    // tokens rather than omitting it.
    let admin = McpClient::with_bearer(&target.base_url, &target.admin_key);
    let stats = extract_structured(
        &admin
            .tools_call("admin.vault.stats", json!({}))
            .await
            .unwrap_or_else(|e| panic!("admin.vault.stats: {} {}", e.code, e.message)),
    );
    let tenants = stats["tenants"].as_array().expect("tenants array");
    let operator_tenant = tenants
        .iter()
        .find(|t| t["tenant_id"] == json!(namespace))
        .unwrap_or_else(|| panic!("operator tenant {namespace} missing from admin.vault.stats: {tenants:?}"));
    let tenant_providers = operator_tenant["providers"].as_array().expect("providers array");
    let slack_stats = tenant_providers
        .iter()
        .find(|p| p["name"] == json!("slack"))
        .unwrap_or_else(|| panic!("slack missing from admin.vault.stats for {namespace}: {tenant_providers:?}"));
    assert_eq!(slack_stats["tokens"], json!(0), "{slack_stats:?}");

    // AC9's proof: the two response bodies captured in the ship trailer.
    println!(
        "AC9 live proof ({}) -- host.vault.status for {namespace}: {}",
        target.label,
        serde_json::to_string_pretty(&status).unwrap_or_else(|_| status.to_string())
    );
    println!(
        "AC9 live proof ({}) -- admin.vault.stats: {}",
        target.label,
        serde_json::to_string_pretty(&stats).unwrap_or_else(|_| stats.to_string())
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
