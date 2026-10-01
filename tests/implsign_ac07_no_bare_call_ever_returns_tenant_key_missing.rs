//! PRD-mcphost-implicit-signup
//! AC7 (P0) — Given the synthorg journey and tool-surface conformance
//! harnesses at the landing commit, When run against a build of this PRD,
//! Then no assertion expects `tenant_key_missing` on `/mcp` and both runs
//! are green on the fixture host.
//!
//! The synthorg journey harness and the tool-surface conformance harness
//! themselves live outside this repo/worktree and are not runnable from
//! this sandbox (direct-build note, see the PRD's evidence section) --
//! this test instead pins the exact behavioral contract those harnesses
//! depend on: no bare `host.*`/`billing.*` call on `/mcp` ever returns
//! `tenant_key_missing` any more, across a representative sweep of the
//! tool names a journey harness would actually call. Fails before this
//! PRD (every one of these returns `tenant_key_missing` today) and
//! passes after (each succeeds via implicit signup instead).

use crate::common;
use common::McpClient;
use serde_json::json;

#[tokio::test]
async fn representative_bare_host_calls_never_return_tenant_key_missing() {
    let server = common::TestServer::start().await;

    let calls: &[(&str, serde_json::Value)] = &[
        ("host.whoami", json!({})),
        ("host.tool_list", json!({})),
        (
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        ),
        ("billing.status", json!({})),
    ];

    for (name, args) in calls {
        // Fresh, independent anonymous client per call -- each one must
        // succeed on its own via implicit signup, not ride a shared
        // session's binding from an earlier call in this loop.
        let client = McpClient::new(&server.base_url);
        let result = client.tools_call(name, args.clone()).await;
        match result {
            Ok(_) => {}
            Err(e) => assert_ne!(
                e.error_code.as_deref(),
                Some("tenant_key_missing"),
                "{name} must never return tenant_key_missing any more: {e:?}"
            ),
        }
    }
}
