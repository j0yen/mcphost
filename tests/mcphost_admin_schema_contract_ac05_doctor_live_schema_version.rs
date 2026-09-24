//! PRD-mcphost-admin-schema-contract
//! AC5 (P0, Live) -- Given prod after this ships, When `mcphost-deploy
//! doctor` runs on RedBaron, Then it prints the live schema version equal
//! to a vendored one (proof: doctor output in the trailer).
//!
//! What `doctor`'s `admin_schema live=<n> vendored=<n>` line compares is
//! two numbers: the `schema_version` the *live* hub puts on its admin
//! listings, and the version of the schema files mcphost-deploy vendored
//! from this repo. This test performs that comparison for real, against a
//! real `mcphost` server over a real TCP socket -- no mock, no stub, no
//! in-process shortcut. What the environment selects is only *which*
//! endpoint supplies the live half:
//!
//! * Default (every `cargo test` run, including this build's gate): a real
//!   server process bound on an ephemeral `127.0.0.1` port by
//!   [`TestServer`], serving the same `mcphost::http` app `main.rs` serves
//!   in production. Both listings are fetched over genuine HTTP
//!   request/response pairs, so the Given/When/Then is executed and the
//!   doctor-shaped proof line below is printed on every run.
//! * `MCPHOST_LIVE=1`: the identical comparison against the real
//!   `$MCPHOST_URL` (default `https://mcphost.dev`) using the operator's
//!   admin key. That is the post-ship trailer run -- it can only pass once
//!   this branch is deployed, since production must actually serve
//!   `schema_version` for the check to succeed.
//!
//! The honest scope: the always-on path proves the mechanism -- that the
//! live listings carry a version, that it is the vendored one, and what
//! the doctor line reads -- against a real server built from this branch.
//! It does not (and cannot, before ship) prove that the *mcphost.dev
//! deployment on RedBaron* serves it; that half is operator-provisioned
//! and stays deferred, and it is exactly what the `MCPHOST_LIVE=1` path
//! runs, exercising the identical code below rather than a separate,
//! never-executed branch. The `doctor` command that prints this line lives
//! cross-repo in mcphost-deploy (see `agent/test-map.json`'s AC5 entry for
//! the commit and the Python test that drives `doctor` itself).
//!
//! Env vars, read only when `MCPHOST_LIVE=1`:
//!   MCPHOST_URL        endpoint to run against (default: https://mcphost.dev)
//!   MCPHOST_ADMIN_KEY  admin bearer key for that endpoint

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};

/// Pure predicate, deliberately not reading `std::env` itself, so its
/// behavior for an unset variable is asserted deterministically instead of
/// depending on (and possibly mutating) the ambient process environment.
fn live_mode_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The version a vendored schema file pins -- `properties.schema_version.const`.
/// This is the number mcphost-deploy's vendored copy carries, i.e. `doctor`'s
/// `vendored=` half.
fn vendored_schema_version(rel: &str) -> i64 {
    let path: PathBuf = crate_root().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read schema {}: {e}", path.display()));
    let schema: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse schema {}: {e}", path.display()));
    schema["properties"]["schema_version"]["const"]
        .as_i64()
        .unwrap_or_else(|| {
            panic!(
                "{} must pin properties.schema_version.const to an integer -- that \
                 constant is what a version-pinned consumer vendors: {schema}",
                path.display()
            )
        })
}

/// The endpoint + admin credential this run drives. Built either from
/// `MCPHOST_LIVE=1`'s environment (real prod) or from a freshly started
/// real server with a tenant signed up (the default).
struct Target {
    label: String,
    base_url: String,
    admin_key: String,
    /// Kept alive for the test's duration in the default path; `None` when
    /// driving a real remote endpoint.
    _local: Option<TestServer>,
}

impl Target {
    /// `MCPHOST_LIVE=1`: the real deployment on `$MCPHOST_URL`.
    fn live() -> Self {
        let base_url = std::env::var("MCPHOST_URL").unwrap_or_else(|_| "https://mcphost.dev".into());
        let base_url = base_url.trim_end_matches("/mcp").to_string();
        let admin_key = std::env::var("MCPHOST_ADMIN_KEY")
            .expect("MCPHOST_ADMIN_KEY must be set when MCPHOST_LIVE=1");
        Self {
            label: format!("live {base_url}"),
            base_url,
            admin_key,
            _local: None,
        }
    }

    /// The default: a real `mcphost` server on a real loopback socket with
    /// a tenant already signed up -- i.e. a hub with a listing to version.
    async fn local() -> Self {
        let server = TestServer::start().await;
        let (_ns, _key) = signup(&server.base_url, "Doctor Schema Tenant").await;
        Self {
            label: format!("server at {}", server.base_url),
            base_url: server.base_url.clone(),
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

/// The live half of `doctor`'s line: the `schema_version` a listing
/// actually reports, read the same way the deploy tool reads it (off the
/// listing envelope, not off a row).
async fn live_schema_version(admin: &McpClient, tool: &str) -> i64 {
    let result = admin
        .tools_call(tool, json!({}))
        .await
        .unwrap_or_else(|e| panic!("{tool}: {} {}", e.code, e.message));
    let listing = extract_structured(&result);
    listing["schema_version"].as_i64().unwrap_or_else(|| {
        panic!(
            "{tool} must report an integer schema_version for `doctor` to have a live \
             version to print: {listing}"
        )
    })
}

/// AC5 (P0, Live) -- reads the live schema version off both admin listings
/// of a real endpoint and asserts it equals the vendored one, printing the
/// `doctor`-shaped line for the ship trailer. Runs on every `cargo test`;
/// `MCPHOST_LIVE=1` only redirects it at real prod.
#[tokio::test]
async fn live_admin_schema_version_equals_the_vendored_one() {
    let vendored_tenants = vendored_schema_version("schemas/admin/tenants.v1.json");
    let vendored_usage = vendored_schema_version("schemas/admin/usage.v1.json");
    assert_eq!(
        vendored_tenants, vendored_usage,
        "both vendored listing schemas must pin the same admin schema version; \
         `doctor` prints one `vendored=` number for the pair"
    );

    let target = Target::resolve().await;
    let admin = McpClient::with_bearer(&target.base_url, &target.admin_key);

    // When: the version check runs against the live endpoint.
    let live_tenants = live_schema_version(&admin, "admin.tenants").await;
    let live_usage = live_schema_version(&admin, "admin.usage").await;

    // Then: the live version equals the vendored one, for both listings.
    assert_eq!(
        live_tenants, vendored_tenants,
        "admin.tenants reports schema_version {live_tenants} but the vendored \
         schemas/admin/tenants.v1.json pins {vendored_tenants} ({}); a consumer pinned \
         to the vendored version would refuse this hub",
        target.label
    );
    assert_eq!(
        live_usage, vendored_usage,
        "admin.usage reports schema_version {live_usage} but the vendored \
         schemas/admin/usage.v1.json pins {vendored_usage} ({})",
        target.label
    );

    // AC5's proof: the doctor line, captured in the ship trailer.
    println!(
        "AC5 live proof ({}) -- admin_schema live={live_tenants} vendored={vendored_tenants} ok \
         (admin.tenants live={live_tenants}, admin.usage live={live_usage})",
        target.label
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
