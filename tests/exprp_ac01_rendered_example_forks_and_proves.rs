//! PRD-mcphost-example-from-published-tool
//! AC1 — Given a fixture tenant with a tool reading `{{ secret.TOKEN }}` and
//! a passing proof call, When `host.example.propose` runs and the rendered
//! example is forked into a fresh tenant that sets `TOKEN`, Then the proof
//! call satisfies `expect` unchanged; a fail is reported as the falsifier
//! firing with the rendered diff.

use crate::exprp;

use crate::common;
use common::{McpClient, extract_structured, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn rendered_example_forked_into_a_fresh_tenant_passes_its_proof_call_unchanged() {
    let f = exprp::start().await;
    exprp::publish_http(&f.client, "probe", exprp::probe_spec(&f.upstream)).await;

    let proposal = exprp::propose(&f.client, exprp::propose_args("probe")).await.unwrap_or_else(|e| panic!("propose: {} {}", e.code, e.message));
    assert_eq!(proposal["status"], "pending");
    assert!(proposal["proposal_id"].as_i64().is_some(), "proposal_id: {proposal}");
    assert_eq!(proposal["secrets_needed"], json!(["TOKEN"]));
    let files = proposal["rendered"]["files"].as_array().expect("rendered.files");
    for want in ["example.toml", "proof.sh", "tools/probe.json"] {
        assert!(files.contains(&json!(want)), "rendered file {want} missing from {files:?}");
    }
    let contents = &proposal["rendered"]["contents"];
    let manifest = contents["example.toml"].as_str().expect("example.toml text");
    assert!(manifest.contains("[provenance]") && manifest.contains(&f.ns), "provenance names the tenant: {manifest}");
    assert!(!manifest.contains(exprp::TOKEN_VALUE), "manifest must not hold the secret value");
    assert!(!contents.to_string().contains(exprp::TOKEN_VALUE), "no rendered file may hold the secret value");

    // Fork: publish the rendered tool file into a fresh tenant that sets TOKEN.
    let spec: Value = serde_json::from_str(contents["tools/probe.json"].as_str().expect("tool file")).expect("tool file is JSON");
    let (_ns_b, key_b) = signup(&f.server.base_url, "exprp fork").await;
    let fork = McpClient::with_bearer(&f.server.base_url, &key_b);
    fork.tools_call("host.secret_set", json!({"name": "TOKEN", "value": exprp::TOKEN_VALUE})).await.expect("fork secret_set");
    fork.tools_call("host.tool_publish", json!({"name": "probe", "kind": "http", "spec": spec}))
        .await
        .expect("fork publish");
    let result = fork
        .tools_call("host.tool_call", json!({"name": "probe", "args": {}}))
        .await
        .unwrap_or_else(|e| panic!("proof call in the fork: {} {}", e.code, e.message));
    let result = extract_structured(&result);
    if let Err(why) = mcphost::example_propose::eval_expect(exprp::PROOF_EXPECT, &result) {
        panic!("FALSIFIER FIRED: {why}\nrendered diff:\n{}", serde_json::to_string_pretty(contents).unwrap_or_default());
    }
}

/// The python render path: a python tool with a declared secret (so it
/// renders as a JSON file carrying `source` + `secrets`) must fork into a
/// fresh tenant and pass its proof call there once that tenant sets TOKEN,
/// and must NOT pass without it (the secret really is read by the bridge).
#[tokio::test]
async fn python_tool_with_a_secret_renders_forks_and_passes_its_proof_call() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let f = exprp::start_with(common::python_kind_registry(&envs_dir.0)).await;
    let expect = "result.probed == 1";
    let source = "import os\ndef main(args):\n    v = os.environ.get('SECRET_TOKEN') or ''\n    return {\"probed\": 1 if (v.startswith('tok_live_') and v.endswith('8841')) else 0}\n";
    let spec = json!({"source": source, "args_schema": {"type": "object"}, "secrets": ["TOKEN"], "description": "Probe with a stored token."});
    f.client
        .tools_call("host.tool_publish", json!({"name": "pyprobe", "kind": "python", "spec": spec}))
        .await
        .unwrap_or_else(|e| panic!("publish pyprobe: {} {}", e.code, e.message));

    let mut args = exprp::propose_args("pyprobe");
    args["proof_call"]["expect"] = json!(expect);
    let proposal = exprp::propose(&f.client, args).await.unwrap_or_else(|e| panic!("propose: {} {}", e.code, e.message));
    assert_eq!(proposal["secrets_needed"], json!(["TOKEN"]));
    let contents = &proposal["rendered"]["contents"];
    assert!(!contents.to_string().contains(exprp::TOKEN_VALUE), "no rendered file may hold the secret value");
    let tool_file = ["tools/pyprobe.json", "tools/pyprobe.py"]
        .into_iter()
        .find(|p| contents[*p].is_string())
        .unwrap_or_else(|| panic!("no python tool file rendered: {contents}"));
    let rendered: Value = if tool_file.ends_with(".json") {
        serde_json::from_str(contents[tool_file].as_str().unwrap()).expect("tool file is JSON")
    } else {
        json!({"source": contents[tool_file], "args_schema": {"type": "object"}})
    };
    assert_eq!(rendered["source"], json!(source), "rendered python source must be the published source");

    let call = |client: McpClient| async move {
        let mut last = None;
        for _ in 0..40 {
            match client.tools_call("host.tool_call", json!({"name": "pyprobe", "args": {}})).await {
                Ok(r) if extract_structured(&r).get("probed").is_some() => return Some(extract_structured(&r)),
                other => last = Some(other),
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        panic!("python proof call never produced a result: {last:?}");
    };

    // Fork without TOKEN: the host refuses the publish (the secret is load-bearing).
    let (_ns_c, key_c) = signup(&f.server.base_url, "exprp py fork no secret").await;
    let bare = McpClient::with_bearer(&f.server.base_url, &key_c);
    let refused = bare
        .tools_call("host.tool_publish", json!({"name": "pyprobe", "kind": "python", "spec": rendered.clone()}))
        .await
        .expect_err("publishing without TOKEN set must be refused");
    assert_eq!(refused.error_code.as_deref(), Some("secret_missing"), "{refused:?}");

    // Fork with TOKEN: the proof passes unchanged.
    let (_ns_b, key_b) = signup(&f.server.base_url, "exprp py fork").await;
    let fork = McpClient::with_bearer(&f.server.base_url, &key_b);
    fork.tools_call("host.secret_set", json!({"name": "TOKEN", "value": exprp::TOKEN_VALUE})).await.expect("fork secret_set");
    fork.tools_call("host.tool_publish", json!({"name": "pyprobe", "kind": "python", "spec": rendered})).await.expect("fork publish");
    let result = call(fork).await.unwrap();
    if let Err(why) = mcphost::example_propose::eval_expect(expect, &result) {
        panic!("FALSIFIER FIRED: {why}\nrendered diff:\n{}", serde_json::to_string_pretty(contents).unwrap_or_default());
    }
}
