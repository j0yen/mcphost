//! PRD-mcphost-empty-call-refusal-stated-in-schema
//! contract: reads registry handler::host_tool_descriptors
//! AC3 (P0) -- Given a `TestServer` over the production `build_router`, When
//! `host.import {}` and `host.key.delegate {}` are called with a valid tenant
//! key, Then each is refused `args_invalid` by the validator with `data.path`
//! the root and `data.expected` equal to the schema's `anyOf` branches, and
//! `handler_runs` for both stays at zero.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::handler::host_tool_descriptors;
use mcphost::host_args::handler_runs;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

#[tokio::test]
async fn ecs_ac03_import_and_delegate_refuse_an_empty_call_in_the_validator() {
    let server = TestServer::start_with_signup_rate_limit(100_000).await;
    let (ns, key) = signup(&server.base_url, "ECS AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    for name in ["host.import", "host.key.delegate"] {
        let schema_any_of = tools
            .iter()
            .find(|t| t.name.as_ref() == name)
            .and_then(|t| t.input_schema.get("anyOf"))
            .cloned()
            .unwrap_or_else(|| panic!("{name}: no anyOf"));
        let before = handler_runs(&ns, name);
        let err = client.tools_call(name, json!({})).await.expect_err("an empty call must be refused");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{name}: {err:?}");
        // The root instance (`""` in JSON-pointer terms) is rendered `/` by the
        // validator, as for every other root-level `anyOf` miss (insch_ac04).
        assert_eq!(err.data.get("path"), Some(&Value::String("/".into())), "{name} data.path");
        assert_eq!(err.data.get("constraint"), Some(&json!("anyOf")), "{name} data.constraint");
        assert_eq!(err.data.get("expected"), Some(&schema_any_of), "{name} data.expected");
        assert_eq!(handler_runs(&ns, name), before, "{name}: the handler must not run");
        assert_eq!(handler_runs(&ns, name), 0, "{name}: handler_runs stays at zero");
    }
}
