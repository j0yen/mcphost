//! AC1 (PRD-mcphost-call-limits-honest) — Given a python tool with
//! `timeout_s: 45` that sleeps 40 s, When called, Then it returns ok after
//! about 40 s; Given `timeout_s: 45` and a 50 s sleep, Then `call_timeout`
//! names a 45 s deadline.
//!
//! Before this PRD, `handler.rs` wrapped every dispatch in the fixed 30s
//! `AppState::call_timeout` regardless of what a spec declared -- a tool
//! declaring `timeout_s: 45` (accepted at publish time, since
//! `MAX_TIMEOUT_S` is 60) was killed at 30s in practice. `Kind::requested_timeout`
//! (this PRD) lets `python`'s own declared `timeout_s` override that
//! default at the dispatch site.
//!
//! Both cases here sleep well under 60s (the real `MAX_TIMEOUT_S`), so
//! there is no need to shrink `AppState::call_timeout` for this test the
//! way `ac15_call_timeout.rs` does for its own (unrelated, kind-agnostic)
//! deadline -- the whole point is proving the *real* 30s default gets
//! overridden by a real 45s declaration.

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn declared_timeout_of_45s_lets_a_40s_sleep_complete() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    // timeout_s 45 is above the free plan's sandbox_cpu_s_max (30): pro.
    let (ns, key, _) = common::signup_and_make_pro(&server, "AC1 Tenant A", "cus_srlp_l1a").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(args.get(\"seconds\", 0))\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 45,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow45", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.slow45");

    // Warm the environment build with a near-instant call first, so the
    // timed call below measures the sandboxed run itself, not env-build
    // latency racing the deadline.
    let _ = poll_until_ready(&client, &qualified, json!({"seconds": 0}), Duration::from_secs(20))
        .await
        .expect("warm call ok");

    let result = crate::deadline_budget!(45_000, at_least = 39_000, {
        client
            .tools_call(&qualified, json!({"seconds": 40}))
            .await
            .expect("a 40s sleep under a 45s declared timeout must succeed")
    });
    let structured = common::extract_structured(&result);
    assert_eq!(structured["ok"], json!(true));
}

#[tokio::test]
async fn declared_timeout_of_45s_names_the_45s_deadline_on_overrun() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    // timeout_s 45 is above the free plan's sandbox_cpu_s_max (30): pro.
    let (ns, key, _) = common::signup_and_make_pro(&server, "AC1 Tenant B", "cus_srlp_l1b").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import time\ndef main(args):\n    time.sleep(args.get(\"seconds\", 0))\n    return {\"ok\": True}\n",
        "args_schema": {"type": "object"},
        "timeout_s": 45,
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "slow45b", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let qualified = format!("{ns}.slow45b");

    let _ = poll_until_ready(&client, &qualified, json!({"seconds": 0}), Duration::from_secs(20))
        .await
        .expect("warm call ok");

    let err = crate::deadline_budget!(50_000, at_least = 44_000, {
        client
            .tools_call(&qualified, json!({"seconds": 50}))
            .await
            .expect_err("a 50s sleep under a 45s declared timeout must time out")
    });
    assert_eq!(err.error_code.as_deref(), Some("call_timeout"));
    assert!(
        err.message.contains("45s"),
        "call_timeout message must name the 45s deadline that actually applied: {}",
        err.message
    );
}
