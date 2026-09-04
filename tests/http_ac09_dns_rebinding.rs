//! AC9 — Given a DNS name that resolves to a private address at call time,
//! When called, Then the error is `host_not_allowed` and no connection is
//! made.
//!
//! This is the DNS-rebinding case requirement 8 exists for: `rebind.example`
//! looks like an ordinary public host at publish time (so the spec
//! publishes fine), but by the time the tool is *called*, its DNS answer
//! has moved to a private address -- exactly what an attacker controlling
//! that domain's DNS could do between publish and call. The
//! [`common::FixedLookup`] stands in for that attacker-controlled DNS
//! answer; the production code path (the real `VettingResolver`, wired as
//! `reqwest`'s actual `dns_resolver`) is what's under test, not a bypassed
//! version of it.

mod common;
use common::{FixedLookup, McpClient, TestServer, http_kind_registry_strict, signup};
use serde_json::json;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

#[tokio::test]
async fn hostname_rebinding_to_a_private_address_is_refused_at_call_time() {
    let mut answers = HashMap::new();
    answers.insert(
        "rebind.example".to_string(),
        vec![IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))],
    );
    let server =
        TestServer::start_with_kinds(http_kind_registry_strict(FixedLookup(answers))).await;
    let (ns, key) = signup(&server.base_url, "Rebind Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // A hostname, not a literal IP -- publishes fine, since `validate`'s
    // syntactic check has no DNS answer yet to judge.
    let spec = json!({
        "method": "GET",
        "url": "https://rebind.example/v1/thing",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "rebind", "kind": "http", "spec": spec}),
        )
        .await
        .expect("a hostname that isn't obviously private must publish fine");

    let start = Instant::now();
    let err = client
        .tools_call(&format!("{ns}.rebind"), json!({}))
        .await
        .expect_err("a hostname resolving to a private address must be refused at call time");
    let elapsed = start.elapsed();

    assert_eq!(err.error_code.as_deref(), Some("host_not_allowed"));
    // 10.1.2.3 is unreachable in this sandbox; if the code had actually
    // attempted a connection instead of refusing at resolve time, this
    // would time out (30s) or fail slowly, not return promptly.
    assert!(
        elapsed < Duration::from_secs(5),
        "the block must happen before any connection attempt, took {elapsed:?}"
    );
}
