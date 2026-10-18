//! PRD-mcphost-error-envelope-schema-contract
//! AC2 (P0) -- Given every canonical verb from `registered_tools()`, When the
//! generated test drives each verb's `args_invalid` path, Then every envelope
//! validates against that verb's registered output schema error branch, the
//! test fails naming the first verb with `data` null or absent, and the
//! `skipped:` list has at most 5 entries.

use crate::common;
use common::{McpClient, TestServer, http_kind_registry, signup};
use mcphost::output_schemas::{EnvelopeProbe, envelope_probes, error_branch_for};
use serde_json::{Value, json};

#[tokio::test]
async fn errenv_ac02() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Errenv AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let anon = McpClient::new(&server.base_url);
    let mut validated = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let probes = envelope_probes();
    assert!(probes.len() > 100, "registry enumeration looks broken: {} verbs", probes.len());
    for (tool, probe) in probes {
        let attempts = match probe {
            EnvelopeProbe::Skipped(reason) => {
                skipped.push(format!("skipped:{tool}:{reason}"));
                continue;
            }
            EnvelopeProbe::Drive(attempts) => attempts,
        };
        let mut envelope: Option<Value> = None;
        for args in attempts {
            let caller = if args.get("tenant_key").is_some() { &anon } else { &client };
            if let Err(e) = caller.tools_call(tool, args).await {
                envelope = Some(json!({"code": e.code, "message": e.message, "data": e.data}));
                break;
            }
        }
        let Some(envelope) = envelope else {
            skipped.push(format!("skipped:{tool}:accepts every probe"));
            continue;
        };
        let data = &envelope["data"];
        if !data.is_object() {
            problems.push(format!("{tool}: envelope data is {data}, not an object: {envelope}"));
            continue;
        }
        let branch = error_branch_for(tool).unwrap_or_else(|e| panic!("{e}"));
        let validator = jsonschema::validator_for(&branch).expect("error branch compiles");
        if !validator.is_valid(&envelope) {
            problems.push(format!("{tool}: envelope {envelope} fails its registered error branch"));
        } else {
            validated += 1;
        }
    }
    println!("error_envelope validated={validated} skipped={skipped:?}");
    assert!(problems.is_empty(), "first failing verb: {}\n{}", problems[0], problems.join("\n"));
    assert!(skipped.len() <= 5, "skipped count {} > 5: {skipped:?}", skipped.len());
}
