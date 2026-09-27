//! PRD-mcphost-shared-tool-spec-readback
//! AC10 (P0) -- Given prod mcphost after deploy, When tenant A publishes a
//! python tool with one `env` entry and shares it to a group containing
//! tenant B with `expose_spec: true`, and tenant B calls
//! `host.tool_spec_shared`, Then B receives the `source` and no `env`, and
//! A's `host.calls.list` shows B's read.
//!
//! Paired hermetically, in-process, on this branch: a real `mcphost`
//! server ([`TestServer`]) with two freshly signed-up tenants standing in
//! for A and B drives the whole two-actor sequence -- real group, real
//! python publish carrying a real `env` entry, real share with
//! `expose_spec: true`, real `host.tool_spec_shared` read, real read-back
//! of the owner's calls listing. No mock, no stub, no live network hop.
//!
//! This control plane has no tool literally named `host.calls.list`; the
//! owner-visible read of the same `calls` rows the PRD's Then clause
//! describes is `host.usage`'s `calls_by_others` map (caller namespace ->
//! call count against this tenant's tools; `src/control.rs`'s `usage`,
//! backed by `Db::calls_by_others`), and that is the surface
//! `host.tool_spec_shared`'s attributed calls-log write targets
//! (`handler.rs`'s `record_call_attributed(owner_id, ..., Some(caller_id))`).
//! Because that write is deliberately detached from the read's response
//! path (AC8's 50-reader p95 budget), the assertion polls for it instead of
//! assuming it has already landed by the time the read call returns.
//!
//! The prod half of this AC (this branch actually reachable at
//! `https://mcphost.dev` by two real tenants) is not this test's job: it is
//! the daemon's live stage, run after deploy.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;

/// The one `env` entry AC10 calls for, and its value. The value is asserted
/// absent from B's response, so it must be a string that could not
/// plausibly appear there for any other reason.
const ENV_KEY: &str = "AC10_PROBE_API_KEY";
const ENV_VALUE: &str = "ac10-env-value-must-never-leave-tenant-a";

const SOURCE: &str = "def main(args):\n    return {\"read_me\": True}\n";

/// A's calls listing: how many calls its own tools have received from
/// `caller_ns` in the last 24h, as `host.usage` reports it.
async fn calls_from(client_a: &McpClient, caller_ns: &str) -> i64 {
    let usage = extract_structured(
        &client_a
            .tools_call("host.usage", json!({"window": "24h"}))
            .await
            .unwrap_or_else(|e| panic!("A's host.usage: {} {}", e.code, e.message)),
    );
    usage["calls_by_others"][caller_ns].as_i64().unwrap_or(0)
}

/// Poll A's calls listing until B's read is in it. The calls-log write for
/// a spec read is detached from the read's response (AC8's p95 budget), so
/// its absence for a moment is expected; its absence after the deadline is
/// the AC failing.
async fn wait_for_call_from(client_a: &McpClient, caller_ns: &str, want_at_least: i64) -> i64 {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = calls_from(client_a, caller_ns).await;
        if got >= want_at_least {
            return got;
        }
        assert!(
            Instant::now() < deadline,
            "A's calls listing never showed B's ({caller_ns}) read: calls_by_others[{caller_ns}] \
             stalled at {got}, wanted >= {want_at_least}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}


/// AC10's Given/When/Then, proven end to end against a real in-process
/// server: A publishes a python tool carrying one `env` entry, shares it to
/// a group containing B with `expose_spec: true`, B reads the spec and gets
/// `source` but no `env`, and A's calls listing (`host.usage`'s
/// `calls_by_others`) shows B's read.
#[tokio::test]
async fn tenant_b_reads_source_without_env_and_tenant_a_sees_the_read() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (ns_a, key_a) = signup(&server.base_url, "AC10 Tenant A").await;
    let (ns_b, key_b) = signup(&server.base_url, "AC10 Tenant B").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    assert_ne!(ns_a, ns_b, "AC10 is a two-tenant flow; A and B must differ");

    // A's calls listing before B reads anything.
    let before = calls_from(&client_a, &ns_b).await;

    // Given: A publishes a python tool with one `env` entry...
    let mut env = serde_json::Map::new();
    env.insert(ENV_KEY.to_string(), json!(ENV_VALUE));
    let spec = json!({
        "source": SOURCE,
        "args_schema": {"type": "object"},
        "env": Value::Object(env),
    });
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "ac10_spec_readback_probe", "kind": "python", "spec": spec}),
        )
        .await
        .unwrap_or_else(|e| panic!("A's host.tool_publish: {} {}", e.code, e.message));

    // ...and shares it to a group containing B with `expose_spec: true`.
    client_a
        .tools_call("host.group.create", json!({"name": "ac10-spec-readback"}))
        .await
        .unwrap_or_else(|e| panic!("A's host.group.create: {} {}", e.code, e.message));
    client_a
        .tools_call(
            "host.group.add",
            json!({"name": "ac10-spec-readback", "namespace": ns_b}),
        )
        .await
        .unwrap_or_else(|e| panic!("A's host.group.add: {} {}", e.code, e.message));
    client_a
        .tools_call(
            "host.tool_share",
            json!({
                "name": "ac10_spec_readback_probe",
                "visibility": "group",
                "group": "ac10-spec-readback",
                "expose_spec": true,
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("A's host.tool_share: {} {}", e.code, e.message));

    // When: B calls host.tool_spec_shared.
    let qualified = format!("{ns_a}.ac10_spec_readback_probe");
    let read = extract_structured(
        &client_b
            .tools_call("host.tool_spec_shared", json!({"tool": qualified.clone()}))
            .await
            .unwrap_or_else(|e| {
                panic!("B's host.tool_spec_shared on {qualified}: {} {}", e.code, e.message)
            }),
    );

    // Then: B receives the source...
    assert_eq!(read["tool"], json!(qualified), "response: {read}");
    assert_eq!(read["kind"], json!("python"), "response: {read}");
    assert_eq!(read["spec"]["source"], json!(SOURCE), "response: {read}");

    // ...and no env, anywhere in the response -- not just absent as a key.
    let spec_out: &Value = &read["spec"];
    assert!(
        spec_out.get("env").is_none(),
        "B's spec must omit env entirely: {spec_out}"
    );
    let read_text = read.to_string();
    assert!(
        !read_text.contains(ENV_KEY),
        "B's response must not carry the env entry's name: {read_text}"
    );
    assert!(
        !read_text.contains(ENV_VALUE),
        "B's response must not carry the env entry's value: {read_text}"
    );

    // And: A's calls listing shows B's read.
    let after = wait_for_call_from(&client_a, &ns_b, before + 1).await;
    assert!(
        after > before,
        "A's calls listing must show at least one more call from B ({ns_b}): {before} -> {after}"
    );
}

/// The negative half, same file: without `expose_spec` (default false), B's
/// read is refused with the exact same shape AC4 asserts --
/// `spec_not_exposed` naming `<owner_ns>.<name>` -- so AC10's "true" and
/// "false" paths are locked side by side.
#[tokio::test]
async fn without_expose_spec_the_read_is_refused_the_same_way_ac4_asserts() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (ns_a, key_a) = signup(&server.base_url, "AC10 Owner (no expose_spec)").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);
    client_a
        .tools_call("host.group.create", json!({"name": "ac10-spec-readback-closed"}))
        .await
        .expect("group.create");

    let (ns_b, key_b) = signup(&server.base_url, "AC10 Sharee (no expose_spec)").await;
    client_a
        .tools_call(
            "host.group.add",
            json!({"name": "ac10-spec-readback-closed", "namespace": ns_b}),
        )
        .await
        .expect("group.add");

    let spec = json!({
        "source": SOURCE,
        "args_schema": {"type": "object"},
    });
    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "ac10_closed_probe", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    // No `expose_spec` at all -- defaults to false.
    client_a
        .tools_call(
            "host.tool_share",
            json!({
                "name": "ac10_closed_probe",
                "visibility": "group",
                "group": "ac10-spec-readback-closed",
            }),
        )
        .await
        .expect("share ok");

    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let qualified = format!("{ns_a}.ac10_closed_probe");
    let err = client_b
        .tools_call("host.tool_spec_shared", json!({"tool": qualified.clone()}))
        .await
        .expect_err("must refuse: expose_spec was never set");
    assert_eq!(err.error_code.as_deref(), Some("spec_not_exposed"));
    assert!(
        err.message.contains(&qualified),
        "spec_not_exposed message must name '{qualified}': {}",
        err.message
    );

    // And A's calls listing does not gain an entry for a refused read.
    let calls = calls_from(&client_a, &ns_b).await;
    assert_eq!(
        calls, 0,
        "a refused read (spec_not_exposed) must not appear in A's calls listing"
    );
}
