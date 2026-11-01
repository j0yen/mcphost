//! PRD-mcphost-schema-shape-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC5 (P0) -- Given `host.lineage.blast_radius`, When its schema is read, Then
//! `change_kind` carries an `enum` of the lowercase spellings from the const
//! `ChangeKind::parse` reads, every enum value parses, and `toolconf-probe` and
//! `DROP` sent to it are refused by the validator with constraint `enum`.

use crate::common;
use crate::vbd_calls;
use common::{McpClient, TestServer, signup};
use mcphost::host_args::handler_runs;
use mcphost::lineage::graph::ChangeKind;
use serde_json::{Value, json};

const TOOL: &str = "host.lineage.blast_radius";

#[test]
fn shp_ac05_change_kind_enum_is_the_parsers_spellings_and_each_parses() {
    let tool = vbd_calls::all_descriptors().into_iter().find(|t| t.name == TOOL).expect("descriptor");
    let schema = Value::Object((*tool.input_schema).clone());
    let listed: Vec<&str> = schema
        .pointer("/properties/change_kind/enum")
        .and_then(Value::as_array)
        .expect("change_kind carries an enum")
        .iter()
        .map(|v| v.as_str().expect("string member"))
        .collect();
    let spellings: Vec<&str> = ChangeKind::SPELLINGS.iter().map(|(s, _)| *s).collect();
    assert_eq!(listed, spellings, "the enum is the const the parser reads");
    assert!(listed.iter().all(|s| *s == s.to_ascii_lowercase()), "lowercase spellings only: {listed:?}");
    for spelling in &listed {
        assert!(ChangeKind::parse(spelling).is_some(), "enum value {spelling:?} does not parse");
    }
    for kind in ChangeKind::SPELLINGS.iter().map(|(_, k)| k) {
        assert!(listed.contains(&kind.as_str()), "{} has no canonical spelling in the enum", kind.as_str());
    }
}

#[tokio::test]
async fn shp_ac05_an_unknown_or_upper_case_change_kind_is_refused_by_the_validator_with_enum() {
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let (ns, key) = signup(&server.base_url, "SHP AC5").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for value in ["toolconf-probe", "DROP"] {
        let err = client.tools_call(TOOL, json!({"id": "table:orders", "change_kind": value})).await.expect_err("refused");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{value}: {err:?}");
        assert_eq!(err.data.get("path"), Some(&json!("/change_kind")), "{value}: data.path");
        assert_eq!(err.data.get("constraint"), Some(&json!("enum")), "{value}: data.constraint");
    }
    assert_eq!(handler_runs(&ns, TOOL), 0, "the handler must not run");
    let _ = client.tools_call(TOOL, json!({"id": "table:orders", "change_kind": "drop"})).await;
    assert_eq!(handler_runs(&ns, TOOL), 1, "a listed spelling reaches the handler");
}
