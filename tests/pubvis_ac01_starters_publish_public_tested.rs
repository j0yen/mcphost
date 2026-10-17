//! PRD-mcphost-publish-visibility-and-tags AC1 (P0) -- Given the python, http
//! and chain starter tools from `host.quickstart` published with
//! `visibility: "public"`, When each publish runs its dry run, Then all three
//! return `tested_version: 1` and the added latency per publish is <= 2 s; a
//! refusal on any of the three is the falsifier firing.

use crate::common;
use common::{FixedLookup, TestServer, chain_and_python_kind_registry, extract_structured, signup};
use mcphost::kinds::http::HttpKind;
use mcphost::sandbox;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn publish_args(client: &common::McpClient, kind: &str) -> Value {
    let qs = extract_structured(
        &client.tools_call("host.quickstart", json!({"kind": kind})).await.expect("quickstart"),
    );
    let args = if kind == "chain" {
        qs["steps"][0]["arguments"].clone()
    } else {
        qs["starter_tool"]["publish_call"]["arguments"].clone()
    };
    assert!(args.is_object(), "{kind} starter publish arguments: {qs}");
    args
}

async fn publish_public(client: &common::McpClient, kind: &str, mut args: Value) -> Value {
    args["visibility"] = json!("public");
    let out = client
        .tools_call("host.tool_publish", args)
        .await
        .unwrap_or_else(|e| panic!("falsifier: public publish of the {kind} starter was refused: {e:?}"));
    extract_structured(&out)
}

#[tokio::test]
async fn pubvis_ac01() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"fact": "cats", "length": 4})))
        .mount(&upstream)
        .await;
    let _env = common::EnvGuard::set("MCPHOST_HTTP_STARTER_URL", &format!("{}/fact", upstream.uri())).await;

    let envs_dir = common::TempDataDir::new();
    let mut kinds = chain_and_python_kind_registry(&envs_dir.0);
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", Arc::new(FixedLookup(HashMap::new())))));
    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "Pubvis AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // python first: the chain starter calls it, so it must exist (public
    // and tested, which is what the chain then resolves against).
    let python = publish_args(&client, "python").await;
    let http = publish_args(&client, "http").await;
    let chain = publish_args(&client, "chain").await;

    for (kind, args) in [("python", python.clone()), ("http", http), ("chain", chain)] {
        let out = publish_public(&client, kind, args).await;
        assert_eq!(out["tested_version"], json!(1), "{kind}: {out}");
        assert_eq!(out["visibility"], json!("public"), "{kind}: {out}");
        assert_eq!(out["stale_test"], json!(false), "{kind}: {out}");
    }

    // Added latency per publish: the same python starter republished with
    // and without the public gate (each republish is a new, untested
    // version, so every measured publish runs a real dry run).
    let perf_args = python;
    let private_args = perf_args.clone();
    let mut public_args = perf_args;
    public_args["visibility"] = json!("public");
    crate::perf_overhead_budget!(
        2000,
        {
            client.tools_call("host.tool_publish", private_args.clone()).await.expect("private publish");
        },
        {
            client.tools_call("host.tool_publish", public_args.clone()).await.expect("public publish");
        }
    );
}
