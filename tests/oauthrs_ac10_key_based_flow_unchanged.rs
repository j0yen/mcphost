//! PRD-mcphost-oauth-resource-server
//! AC10 (P0) — Given the existing key-based test suites, When they run
//! against this build, Then they pass unchanged. This file is the
//! oauthrs-prefixed proof point; it exercises the ordinary signup ->
//! publish -> call -> admin path end to end with no bearer JWT anywhere
//! in it.
//!
//! The rest of the proof is that none of the pre-existing suites
//! exercising that key-based path needed a single line changed:
//! `ac02_signup_creates_hashed_tenant.rs`, `ac03_tenant_lists_control_plane_only.rs`,
//! `ac04_publish_list_and_call.rs`, `ac05_cross_tenant_isolation.rs`,
//! `ac06_remove_tool.rs`, `ac07_usage_metering.rs`,
//! `ac08_admin_disable_and_forbidden.rs`, `ac09_signup_rate_limit.rs`, and
//! friends are byte-for-byte identical to `main` (`git diff main..HEAD --
//! tests/ac0*.rs` is empty for every one of them) and still pass.
//!
//! Two other classes of file *do* have diffs, and both are orthogonal to
//! "does key-based auth still work":
//! - `ac01_unauthenticated_lists_signup.rs`,
//!   `compat_ac01_ac02_ac03_cache_fields.rs`,
//!   `compat_ac04_ac05_admin_and_tenant_cache_fields.rs`,
//!   `compat_ac11_ac12_claude_sdk_replay.rs`, and
//!   `sessionkey_ac02_ac03_discovery_shape.rs` hardcode the total
//!   tool-list count. Requirement 3 of this PRD makes `host.oauth.issuer_set`/
//!   `issuer_remove`/`issuers` unconditionally discoverable, same as every
//!   other `host.*` tool, so that count genuinely grew by 3 -- these
//!   assertions were updated to the new, correct count the same way every
//!   prior tool-adding PRD in this repo's history has updated them (see
//!   each file's own comment trail), not because a key-based flow broke.
//! - `runs_ac11_admin_runs_reap.rs` and `sched_ac5_overlap_skips_and_records.rs`
//!   build a bare `AppState` directly instead of through
//!   `common::TestServer`, so any new `AppState` field previously forced an
//!   edit there purely to keep the struct literal exhaustive -- unrelated
//!   to what either test is about. Both now delegate to the shared
//!   `common::bare_app_state()` builder (see `tests/common/mod.rs`), so
//!   this PRD's 3 new fields (and every future one) are wired in exactly
//!   once, and these two files need no further changes.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, publish, signup};
use serde_json::json;

#[tokio::test]
async fn key_based_signup_publish_call_and_admin_still_work() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Key Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let qualified = publish(
        &client,
        "hello",
        "echo",
        json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}}),
    )
    .await;
    assert_eq!(qualified, format!("{ns}.hello"));

    let result = client
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("calling a key-authenticated tenant's own published tool must still work");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["msg"], json!("hi"));

    let whoami = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami via key must still succeed");
    let whoami = common::extract_structured(&whoami);
    assert_eq!(whoami["tenant"], json!(ns));
    assert_eq!(whoami["subject"], serde_json::Value::Null, "a key-authenticated caller has no OAuth subject");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let listed = admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants via the admin key must still succeed");
    let listed = common::extract_structured(&listed);
    let tenants = listed["tenants"].as_array().expect("tenants array");
    assert!(
        tenants.iter().any(|t| t["tenant"] == json!(ns)),
        "admin.tenants must still list the key-based tenant: {tenants:?}"
    );
}
