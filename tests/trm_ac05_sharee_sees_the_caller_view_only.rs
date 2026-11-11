//! PRD-mcphost-tool-reach-manifest-v2
//! AC5 (P0) -- Given tenant B and A's tool shared with `expose_spec: true`,
//! When B calls `host.tool.reach`, Then only `hosts`, `dynamic_hosts`,
//! `network` and `secrets_count` return, with no secret name, state key or
//! source; without `expose_spec` the error is `spec_not_exposed`, a share
//! without `read_source` is `scope_missing`, and an unshared name is
//! `tool_not_found`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::sandbox;
use serde_json::{Value, json};

const GROUP: &str = "trm-ac05-friends";

async fn ok(client: &McpClient, verb: &str, args: Value) -> Value {
    extract_structured(
        &client.tools_call(verb, args).await.unwrap_or_else(|e| panic!("{verb}: {} {}", e.code, e.message)),
    )
}

async fn refused(client: &McpClient, name: &str) -> common::RpcError {
    client.tools_call("host.tool.reach", json!({"name": name})).await.expect_err("must be refused")
}

#[tokio::test]
async fn trm_ac05_a_sharee_reads_hosts_network_and_a_secret_count_only() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let server = TestServer::start_with_kinds(mcphost::kinds::production_registry()).await;
    let (ns_a, key_a) = signup(&server.base_url, "TRM AC5 Owner").await;
    let (ns_b, key_b) = signup(&server.base_url, "TRM AC5 Sharee").await;
    let (_ns_c, key_c) = signup(&server.base_url, "TRM AC5 Stranger").await;
    let (a, b, c) = (
        McpClient::with_bearer(&server.base_url, &key_a),
        McpClient::with_bearer(&server.base_url, &key_b),
        McpClient::with_bearer(&server.base_url, &key_c),
    );

    for secret in ["acme_pay_key", "acme_org_id"] {
        ok(&a, "host.secret.set", json!({"name": secret, "value": "trm-ac5-secret-value"})).await;
    }
    let http = json!({"method": "GET", "url": "https://api.acme-pay.example/v1/balance",
                      "headers": {"Authorization": "Bearer {{ secret.acme_pay_key }}", "X-Org": "{{ secret.acme_org_id }}"}});
    let python = json!({"secrets": ["acme_pay_key"],
                        "source": "import mcphost\n\ndef main(args):\n    mcphost.state.set(\"ledger/entries\", args)\n    return {}\n"});
    for (name, kind, spec) in [
        ("pay", "http", http.clone()),
        ("ledger", "python", python),
        ("probe", "http", http.clone()),
        ("private_one", "http", http),
    ] {
        ok(&a, "host.tool.publish", json!({"name": name, "kind": kind, "spec": spec})).await;
    }
    ok(&a, "host.group.create", json!({"name": GROUP})).await;
    ok(&a, "host.group.add", json!({"name": GROUP, "namespace": ns_b})).await;
    let share = |name: &'static str, extra: Value| {
        let a = &a;
        async move {
            let mut args = json!({"name": name, "visibility": "group", "group": GROUP});
            args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            ok(a, "host.tool.share", args).await;
        }
    };
    share("pay", json!({"expose_spec": true})).await;
    share("ledger", json!({"expose_spec": true})).await;
    // `probe` is shared twice (the plan allows three shared tools): first
    // without expose_spec, then with expose_spec but without read_source.
    share("probe", json!({})).await;

    // B reads A's shared http tool: exactly the caller view.
    let pay = ok(&b, "host.tool.reach", json!({"name": format!("{ns_a}.pay")})).await;
    assert_eq!(pay["name"], json!(format!("{ns_a}.pay")));
    assert_eq!(pay["kind"], json!("http"));
    let view = pay["reach"].as_object().expect("reach object");
    let mut keys: Vec<&str> = view.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["dynamic_hosts", "hosts", "network", "secrets_count"], "{pay}");
    assert_eq!(pay["reach"]["hosts"], json!(["api.acme-pay.example"]));
    assert_eq!(pay["reach"]["dynamic_hosts"], json!(false));
    assert_eq!(pay["reach"]["network"], json!("none"));
    assert_eq!(pay["reach"]["secrets_count"], json!(2));

    // B reads the python tool: its secret and state key never appear.
    let ledger = ok(&b, "host.tool.reach", json!({"name": format!("{ns_a}.ledger")})).await;
    assert_eq!(ledger["reach"], json!({"hosts": [], "dynamic_hosts": false, "network": "none", "secrets_count": 1}));
    for answer in [&pay, &ledger] {
        let text = answer.to_string();
        for leaked in ["acme_pay_key", "acme_org_id", "ledger/entries", "import mcphost", "trm-ac5-secret-value"] {
            assert!(!text.contains(leaked), "a sharee's answer carries {leaked}: {text}");
        }
    }

    // The owner still gets the whole manifest.
    let own = ok(&a, "host.tool.reach", json!({"name": "pay"})).await;
    assert_eq!(own["reach"]["secrets"], json!(["acme_org_id", "acme_pay_key"]));

    // Without expose_spec, without read_source, unshared, unknown, and not in the group.
    let hidden = refused(&b, &format!("{ns_a}.probe")).await;
    assert_eq!(hidden.error_code.as_deref(), Some("spec_not_exposed"), "{hidden:?}");
    share("probe", json!({"expose_spec": true, "scopes": ["call"]})).await;
    let noscope = refused(&b, &format!("{ns_a}.probe")).await;
    assert_eq!(noscope.error_code.as_deref(), Some("scope_missing"), "{noscope:?}");
    assert_eq!(noscope.data["required"], json!("read_source"));
    let private = refused(&b, &format!("{ns_a}.private_one")).await;
    assert_eq!(private.error_code.as_deref(), Some("tool_not_found"), "{private:?}");
    let unknown = refused(&b, &format!("{ns_a}.no_such_tool")).await;
    assert_eq!(unknown.error_code.as_deref(), Some("tool_not_found"), "{unknown:?}");
    assert_eq!(private.data["hint"], unknown.data["hint"], "an unshared tool looks the same as an unknown one");
    let stranger = refused(&c, &format!("{ns_a}.pay")).await;
    assert_eq!(stranger.error_code.as_deref(), Some("tool_not_found"), "{stranger:?}");
}
