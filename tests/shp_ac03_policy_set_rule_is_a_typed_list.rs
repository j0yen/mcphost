//! PRD-mcphost-schema-shape-agreement
//! contract: reads registry handler::host_tool_descriptors
//! AC3 (P0) -- Given a `TestServer` over `build_router`, When `host.policy.set` gets
//! `rule: "toolconf-probe"` and then a one-element list whose item is valid under
//! the new item schema (an `op` from `RuleOp`), Then the first is refused by the
//! validator at `/rule` with constraint `type` and `handler_runs` stays 0, and the
//! second reaches the handler.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::host_args::handler_runs;
use mcphost::rowpolicy::policy::{PolicyRule, RuleOp};
use serde_json::json;

const TOOL: &str = "host.policy.set";

#[tokio::test]
async fn shp_ac03_a_string_rule_is_refused_by_the_validator_and_a_typed_list_reaches_the_handler() {
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let (ns, key) = signup(&server.base_url, "SHP AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let err = client
        .tools_call(TOOL, json!({"target": {"table": "orders"}, "rule": "toolconf-probe"}))
        .await
        .expect_err("a string rule is refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(err.data.get("path"), Some(&json!("/rule")), "data.path");
    assert_eq!(err.data.get("constraint"), Some(&json!("type")), "data.constraint");
    assert_eq!(handler_runs(&ns, TOOL), 0, "the handler must not run for a string rule");

    let op = RuleOp::NAMES[0];
    let rule = json!([{"column_or_attr": "owner", "op": op, "value": {"literal": "x"}}]);
    let _ = client.tools_call(TOOL, json!({"target": {"table": "orders"}, "rule": rule})).await;
    assert_eq!(handler_runs(&ns, TOOL), 1, "a list valid under the item schema reaches the handler");
}

#[test]
fn shp_ac03_every_advertised_op_deserialises_as_a_rule_op() {
    for op in RuleOp::NAMES {
        let rule: PolicyRule = serde_json::from_value(json!({"column_or_attr": "c", "op": op, "value": {"attr": "a"}}))
            .unwrap_or_else(|e| panic!("op {op}: {e}"));
        assert_eq!(rule.op.as_str(), op);
    }
    assert_eq!(RuleOp::NAMES.len(), 2, "RuleOp gained a variant: add its spelling to RuleOp::NAMES");
}
