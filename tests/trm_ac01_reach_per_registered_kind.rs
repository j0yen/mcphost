//! PRD-mcphost-tool-reach-manifest-v2
//! AC1 (P0) -- Given one fixture tool per registered kind (http with 2
//! `{{ secret.* }}` refs and a literal host, chain of 2 http steps, python
//! writing `orders/` and reading `cfg/` through `mcphost.state`, js with
//! `network: "public"`, wasm, jq, echo), When `host.tool.reach` is called on
//! each, Then the manifest holds the expected hosts, secrets, state keys and
//! modules, derived from the stored spec alone, and no fixture runs.
//!
//! "No fixture runs" is proved two ways: the python and js fixtures write a
//! `mcphost.state` key when they run, and that key is absent afterwards; and
//! `host.usage` counts no call for any fixture.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup, wasm_fixture_b64};
use mcphost::sandbox;
use serde_json::{Value, json};

async fn reach(client: &McpClient, name: &str) -> Value {
    let result = client
        .tools_call("host.tool.reach", json!({"name": name}))
        .await
        .unwrap_or_else(|e| panic!("host.tool.reach {name}: {} {}", e.code, e.message));
    extract_structured(&result)
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap_or_else(|| panic!("not an array: {v}")).iter().map(|s| s.as_str().expect("string").to_string()).collect()
}

#[tokio::test]
async fn trm_ac01_manifest_per_registered_kind_derived_from_the_stored_spec() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let server = TestServer::start_with_kinds(mcphost::kinds::production_registry()).await;
    let (_ns, key) = signup(&server.base_url, "TRM AC1 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for secret in ["stripe", "org_id"] {
        client.tools_call("host.secret.set", json!({"name": secret, "value": "trm-ac1-value-not-in-any-manifest"})).await.expect("secret set");
    }
    let publish = |name: &'static str, kind: &'static str, spec: Value| {
        let client = &client;
        async move {
            client
                .tools_call("host.tool.publish", json!({"name": name, "kind": kind, "spec": spec}))
                .await
                .unwrap_or_else(|e| panic!("publish {name}: {} {}", e.code, e.message));
        }
    };

    publish(
        "stripe_balance",
        "http",
        json!({"method": "GET", "url": "https://api.stripe.com/v1/balance",
               "headers": {"Authorization": "Bearer {{ secret.stripe }}", "X-Org": "{{ secret.org_id }}"}}),
    )
    .await;
    publish("rates", "http", json!({"method": "GET", "url": "https://rates.example.com/v1/latest"})).await;
    publish(
        "balance_then_rates",
        "chain",
        json!({"steps": [{"tool": "stripe_balance", "args": {}}, {"tool": "rates", "args": {}}]}),
    )
    .await;
    publish(
        "py_orders",
        "python",
        json!({"source": "import mcphost\n\ndef main(args):\n    mcphost.state.set(\"orders/ran\", True)\n    mcphost.state.set(\"orders/latest\", args)\n    return {\"cfg\": mcphost.state.get(\"cfg/limit\")}\n"}),
    )
    .await;
    publish(
        "js_fetcher",
        "js",
        json!({"network": "public", "source": "function main(args) { mcphost.state.set('orders/ran-js', true); return mcphost.state.get('cfg/js'); }\n"}),
    )
    .await;
    publish("wasm_echo", "wasm", json!({"component": wasm_fixture_b64("echo")})).await;
    publish("jq_id", "jq", json!({"program": "."})).await;
    publish("plain_echo", "echo", json!({"schema": {"type": "object"}})).await;

    // http: the literal host and the two secret names, nothing else.
    let http = reach(&client, "stripe_balance").await;
    assert_eq!(http["kind"], json!("http"));
    assert_eq!(http["tool_version"], json!(1));
    assert_eq!(strings(&http["reach"]["hosts"]), ["api.stripe.com"]);
    assert_eq!(strings(&http["reach"]["secrets"]), ["org_id", "stripe"]);
    assert_eq!(http["reach"]["dynamic_hosts"], json!(false));
    assert_eq!(http["reach"]["network"], json!("none"));
    assert_eq!(http["reach"]["state"], json!({"reads": [], "writes": [], "tables": []}));
    assert!(!http.to_string().contains("trm-ac1-value-not-in-any-manifest"), "no secret value in a manifest: {http}");

    // chain: its steps by name, and the union of the two steps' reach.
    let chain = reach(&client, "balance_then_rates").await;
    assert_eq!(chain["kind"], json!("chain"));
    assert_eq!(strings(&chain["reach"]["calls"]), ["rates", "stripe_balance"]);
    assert_eq!(strings(&chain["reach"]["hosts"]), ["api.stripe.com", "rates.example.com"]);
    assert_eq!(strings(&chain["reach"]["secrets"]), ["org_id", "stripe"]);

    // python: state keys and the module, no host.
    let py = reach(&client, "py_orders").await;
    assert_eq!(strings(&py["reach"]["state"]["writes"]), ["orders/latest", "orders/ran"]);
    assert_eq!(strings(&py["reach"]["state"]["reads"]), ["cfg/limit"]);
    assert_eq!(py["reach"]["state"]["tables"], json!([]));
    assert_eq!(strings(&py["reach"]["modules"]), ["state"]);
    assert_eq!(py["reach"]["hosts"], json!([]));
    assert_eq!(py["reach"]["network"], json!("none"));
    assert_eq!(py["reach"]["dynamic_hosts"], json!(false));

    // js with network: public: the same scan, and the hosts are not knowable.
    let js = reach(&client, "js_fetcher").await;
    assert_eq!(js["kind"], json!("js"));
    assert_eq!(strings(&js["reach"]["state"]["writes"]), ["orders/ran-js"]);
    assert_eq!(strings(&js["reach"]["state"]["reads"]), ["cfg/js"]);
    assert_eq!(js["reach"]["network"], json!("public"));
    assert_eq!(js["reach"]["dynamic_hosts"], json!(true));
    assert_eq!(js["reach"]["hosts"], json!([]));

    // wasm, jq and echo: an empty manifest.
    for name in ["wasm_echo", "jq_id", "plain_echo"] {
        let empty = reach(&client, name).await;
        assert_eq!(
            empty["reach"],
            json!({"hosts": [], "dynamic_hosts": false, "secrets": [],
                   "state": {"reads": [], "writes": [], "tables": []},
                   "modules": [], "calls": [], "network": "none"}),
            "{name}: {empty}"
        );
    }

    // No fixture ran: neither sandboxed tool wrote its marker key, no call was metered.
    for marker in ["orders/ran", "orders/ran-js", "orders/latest"] {
        let got = client.tools_call("host.state.get", json!({"key": marker})).await;
        let found = got.as_ref().ok().map(extract_structured).is_some_and(|v| !v["value"].is_null() && v["found"] != json!(false));
        assert!(!found, "{marker} was written, so a fixture ran: {got:?}");
    }
    let usage = extract_structured(&client.tools_call("host.usage", json!({"window": "24h"})).await.expect("host.usage"));
    assert_eq!(usage["calls"].as_i64(), Some(0), "reading reach must not run or meter a tool: {usage}");
}
