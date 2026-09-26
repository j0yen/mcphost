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
//! AC9's Then names one artifact by path: "both transcripts saved under
//! docs/receipts/<slug>.md" plus the post-deploy healthz version. That file
//! is committed at `docs/receipts/mcphost-upstream-token-vault-status.md`,
//! and it is not prose a reader has to trust: the branch-local transcript
//! block in it is compared, byte for byte, against what a real server built
//! from this branch actually serves
//! ([`receipt_records_the_transcripts_the_server_actually_serves`]), with
//! only the tenant namespace and the crate version replaced by named
//! placeholders. So the receipt cannot drift from the code, and reverting
//! the `LEFT JOIN` in [`mcphost::db::Db::vault_stats`] fails the comparison
//! as loudly as it fails the assertions below. The receipt's *prod* section
//! is explicitly marked pending -- it is what the operator's
//! `MCPHOST_LIVE=1` run appends, and is the half that stays deferred.
//!
//! Env vars, read only when `MCPHOST_LIVE=1`:
//!   MCPHOST_URL           endpoint to run against (default: https://mcphost.dev)
//!   MCPHOST_OPERATOR_KEY  bearer key for the operator tenant (mcphost-1 /etc/mcphost/operator-tenant.key)
//!   MCPHOST_ADMIN_KEY     the admin key (orch ~/.config/mcphost/admin-key)

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};

/// AC9's own named evidence artifact, relative to the crate root.
const RECEIPT_REL: &str = "docs/receipts/mcphost-upstream-token-vault-status.md";

/// The two values the committed transcript deliberately does not freeze:
/// the stand-in operator tenant's namespace (freshly generated per run) and
/// the crate version (bumped by every release commit). Everything else in
/// the block is compared literally.
const TENANT_PLACEHOLDER: &str = "<operator-tenant>";
const VERSION_PLACEHOLDER: &str = "<crate version>";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn receipt_text() -> String {
    let path = repo_root().join(RECEIPT_REL);
    fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e} -- AC9's Then names this file as the artifact its two transcripts \
             are saved under, so it must exist on the branch, not only after the ship trailer \
             runs",
            path.display()
        )
    })
}

/// `GET /healthz` with the admin bearer -- the full document, which is the
/// only shape that carries `version`/`db_ok` (the anonymous body is
/// `{"ok": bool}` and nothing else, per PRD-mcphost-healthz-minimal
/// requirement 1, so the version AC9's evidence line names is unreadable
/// without the admin bearer).
async fn fetch_healthz(base_url: &str, admin_key: &str) -> Value {
    let resp = reqwest::Client::new()
        .get(format!("{base_url}/healthz"))
        .bearer_auth(admin_key)
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {base_url}/healthz: {e}"));
    resp.json()
        .await
        .unwrap_or_else(|e| panic!("GET {base_url}/healthz body is not JSON: {e}"))
}

/// The receipt's branch-local transcript block, rendered from live response
/// bodies. Deterministic given the state AC9's Given describes (one tenant,
/// one registered-never-connected `slack` provider), because the only
/// run-varying values -- the namespace and the crate version -- are replaced
/// by [`TENANT_PLACEHOLDER`] / [`VERSION_PLACEHOLDER`].
fn canonical_transcript(namespace: &str, healthz: &Value, status: &Value, stats: &Value) -> String {
    assert!(
        !namespace.is_empty(),
        "an empty namespace would make the placeholder substitution meaningless"
    );
    let redact = |v: &Value| -> String {
        serde_json::to_string_pretty(v)
            .expect("response bodies serialize")
            .replace(namespace, TENANT_PLACEHOLDER)
    };
    // Only the two fields AC9's evidence line actually names -- the rest of
    // the admin healthz document is volatile box telemetry (counts, disk
    // figures) that would make this block unstable for no added proof.
    let healthz_evidence = json!({
        "db_ok": healthz.get("db_ok").cloned().unwrap_or(Value::Null),
        "version": VERSION_PLACEHOLDER,
    });
    let mut out = String::new();
    out.push_str("`GET /healthz` (admin bearer):\n\n```json\n");
    out.push_str(&redact(&healthz_evidence));
    out.push_str("\n```\n\n");
    out.push_str("`host.vault.status {\"end_user\": \"vaultst-probe\"}` (operator key):\n\n```json\n");
    out.push_str(&redact(status));
    out.push_str("\n```\n\n");
    out.push_str("`admin.vault.stats {}` (admin key):\n\n```json\n");
    out.push_str(&redact(stats));
    out.push_str("\n```");
    out
}

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

    // And: the version AC9's evidence line pairs with those two transcripts
    // ("healthz version after deploy"), read the way the live trailer reads
    // it -- the admin-bearer healthz document, since the anonymous body
    // carries no version at all.
    let healthz = fetch_healthz(&target.base_url, &target.admin_key).await;
    assert_eq!(healthz["db_ok"], json!(true), "{healthz:?}");
    let version = healthz["version"]
        .as_str()
        .unwrap_or_else(|| panic!("admin GET /healthz carries no version field: {healthz:?}"))
        .to_string();

    // AC9's proof: the three bodies captured in the ship trailer, in the
    // shape docs/receipts/mcphost-upstream-token-vault-status.md records
    // them, so an operator's MCPHOST_LIVE=1 run can paste the block straight
    // into that file's prod section.
    println!(
        "AC9 live proof ({}) -- version {version}\n{}",
        target.label,
        canonical_transcript(&namespace, &healthz, &status, &stats)
    );
}

/// AC9's Then, branch-local half: the transcript block committed in
/// `docs/receipts/mcphost-upstream-token-vault-status.md` is what a real
/// server built from this branch actually serves for AC9's Given, not a
/// hand-written sample. Always local -- comparing a committed branch
/// transcript against a remote deployment's body would be comparing two
/// different things, so this deliberately does not follow `MCPHOST_LIVE`.
#[tokio::test]
async fn receipt_records_the_transcripts_the_server_actually_serves() {
    let target = Target::local().await;
    let operator = McpClient::with_bearer(&target.base_url, &target.operator_key);
    let admin = McpClient::with_bearer(&target.base_url, &target.admin_key);

    let whoami = extract_structured(
        &operator
            .tools_call("host.whoami", json!({}))
            .await
            .unwrap_or_else(|e| panic!("host.whoami: {} {}", e.code, e.message)),
    );
    let namespace = whoami["namespace"].as_str().expect("namespace field").to_string();

    let status = extract_structured(
        &operator
            .tools_call("host.vault.status", json!({"end_user": "vaultst-probe"}))
            .await
            .unwrap_or_else(|e| panic!("host.vault.status: {} {}", e.code, e.message)),
    );
    let stats = extract_structured(
        &admin
            .tools_call("admin.vault.stats", json!({}))
            .await
            .unwrap_or_else(|e| panic!("admin.vault.stats: {} {}", e.code, e.message)),
    );
    let healthz = fetch_healthz(&target.base_url, &target.admin_key).await;

    // The placeholder the receipt carries stands for *this* crate version,
    // which is what makes substituting it honest rather than a hole.
    assert_eq!(
        healthz["version"],
        json!(env!("CARGO_PKG_VERSION")),
        "admin healthz must report this build's own version: {healthz:?}"
    );

    let block = canonical_transcript(&namespace, &healthz, &status, &stats);
    let receipt = receipt_text();
    assert!(
        receipt.contains(&block),
        "{RECEIPT_REL}'s branch-local transcript does not match what the server serves \
         for AC9's Given. Replace that block with:\n\n{block}\n\nReceipt currently reads:\n\n{receipt}"
    );
}

/// The receipt is AC9's evidence artifact, so it has to say what AC9's Then
/// says -- and it has to obey AC9's own guardrail ("no key or secret text").
/// A receipt that quietly pasted a bearer token in would satisfy the
/// transcript comparison above and still be unpublishable.
#[test]
fn receipt_names_ac9s_evidence_and_leaks_no_key_or_secret() {
    let receipt = receipt_text();

    for needle in [
        "host.vault.status",
        "admin.vault.stats",
        "/healthz",
        "vaultst-probe",
        "mcphost-1",
        "https://mcphost.dev/mcp",
        "tests/vaultst_ac09_live_vault_status_trailer.rs",
    ] {
        assert!(
            receipt.contains(needle),
            "{RECEIPT_REL} does not mention {needle:?}, which AC9's Given/When/Then names"
        );
    }
    assert!(
        receipt.to_lowercase().contains("pending"),
        "{RECEIPT_REL} must mark AC9's prod leg as still pending the operator's run rather \
         than reading as though prod had already been verified from this sandbox"
    );

    // AC9's guardrail. `ADMIN_KEY` is the suite's own admin bearer and the
    // placeholder credentials are what the stand-in tenant registers; none
    // of them belong in a committed, publicly rendered receipt (this file is
    // concatenated into www/llms-full.txt by scripts/gen-llms-full.sh).
    for secret in [
        ADMIN_KEY,
        "placeholder-client-secret",
        "placeholder-client-id",
    ] {
        assert!(
            !receipt.contains(secret),
            "{RECEIPT_REL} contains {secret:?} -- AC9 requires no key or secret text in the \
             saved evidence"
        );
    }
    // A bearer header is allowed to *appear* in the operator's copy-paste
    // command, but only ever reading a key out of the environment -- never
    // with a literal token after it. Checking the header's presence alone
    // would force the runnable command out of the receipt for no gain.
    for (idx, _) in receipt.match_indices("Authorization: Bearer ") {
        let rest = &receipt[idx + "Authorization: Bearer ".len()..];
        assert!(
            rest.starts_with('$'),
            "{RECEIPT_REL} has an Authorization: Bearer header followed by something other \
             than a shell variable -- AC9 requires no key text in the saved evidence: \
             {:?}",
            &rest[..rest.len().min(40)]
        );
    }
    assert!(
        !receipt.contains("client_secret\":"),
        "{RECEIPT_REL} carries a client_secret field -- neither the status nor the stats body \
         emits one, so its presence means a provider row got pasted in raw"
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
