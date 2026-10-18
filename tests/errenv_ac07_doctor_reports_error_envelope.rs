//! PRD-mcphost-error-envelope-schema-contract
//! AC7 (P1) -- Given `host.doctor`, When run, Then it reports
//! `error_envelope: {validated: N, skipped: [...]}` from the same enumeration
//! as AC2.

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use mcphost::output_schemas::{EnvelopeProbe, envelope_probes, registered_tools};
use serde_json::json;

#[tokio::test]
async fn errenv_ac07() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Errenv AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let doctor = extract_structured(&client.tools_call("host.doctor", json!({})).await.expect("host.doctor"));
    let report = &doctor["error_envelope"];
    let validated = report["validated"].as_u64().unwrap_or_else(|| panic!("no validated count: {doctor}")) as usize;
    let skipped: Vec<&str> = report["skipped"]
        .as_array()
        .unwrap_or_else(|| panic!("no skipped list: {doctor}"))
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    // The same enumeration AC2 drives: every registered verb is either
    // validated or named in `skipped`.
    let probes = envelope_probes();
    assert_eq!(validated + skipped.len(), registered_tools().len(), "{report}");
    let static_skips: Vec<String> = probes
        .iter()
        .filter_map(|(tool, p)| match p {
            EnvelopeProbe::Skipped(reason) => Some(format!("skipped:{tool}:{reason}")),
            EnvelopeProbe::Drive(_) => None,
        })
        .collect();
    for s in &static_skips {
        assert!(skipped.contains(&s.as_str()), "{s} missing from doctor's skipped list {skipped:?}");
    }
    assert!(validated > 200, "{report}");
    assert!(skipped.len() <= 5, "{report}");
}
