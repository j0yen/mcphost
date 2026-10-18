//! PRD-mcphost-input-schema-is-the-validator
//! AC4 (P0) -- Given a `host.*` call whose args violate the descriptor, When
//! dispatched, Then the response is `args_invalid` whose `data` carries
//! `path`, `constraint`, `expected` and `docs`, and a counter in the handler
//! body proves it never ran for that call.

use crate::common;
use common::{McpClient, TestServer, signup};
use mcphost::host_args::handler_runs;
use serde_json::{Value, json};

struct Case {
    tool: &'static str,
    args: Value,
    constraint: &'static str,
    path: &'static str,
}

#[tokio::test]
async fn violating_host_calls_are_refused_by_schema_before_the_handler_runs() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Insch AC4").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let cases = [
        // `anyOf`: neither id nor name.
        Case { tool: "host.docs.get", args: json!({}), constraint: "anyOf", path: "/" },
        // `required`: a plain required argument absent.
        Case { tool: "host.notify.send", args: json!({"text": "hi"}), constraint: "required", path: "/level" },
        // `enum`.
        Case { tool: "host.notify.send", args: json!({"text": "hi", "level": "loud"}), constraint: "enum", path: "/level" },
        // `minimum`.
        Case {
            tool: "host.task.post",
            args: json!({"to": "@x", "payload": {}, "deadline_s": 5}),
            constraint: "minimum",
            path: "/deadline_s",
        },
        // `maxItems`.
        Case {
            tool: "host.ask.create",
            args: json!({"question": "q", "options": ["1", "2", "3", "4", "5", "6", "7"]}),
            constraint: "maxItems",
            path: "/options",
        },
        // `pattern`.
        Case {
            tool: "host.index.create",
            args: json!({"name": "Bad Name", "sources": [{"kind": "doc", "prefix": "a"}]}),
            constraint: "pattern",
            path: "/name",
        },
    ];
    for case in cases {
        let before = handler_runs(&ns, case.tool);
        let err = client
            .tools_call(case.tool, case.args.clone())
            .await
            .expect_err(&format!("{} {} must be refused", case.tool, case.args));
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{}: {err:?}", case.tool);
        let data = &err.data;
        assert_eq!(data["constraint"], case.constraint, "{} data: {data}", case.tool);
        assert_eq!(data["path"], case.path, "{} data: {data}", case.tool);
        assert!(!data["expected"].is_null(), "{} carries no `expected`: {data}", case.tool);
        assert!(data["docs"].as_str().is_some_and(|d| d.contains("tools.md")), "{} docs: {data}", case.tool);
        assert_eq!(handler_runs(&ns, case.tool), before, "{}: the handler ran for a refused call", case.tool);
    }
}

#[tokio::test]
async fn a_schema_valid_call_does_reach_the_handler() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Insch AC4 ok").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let before = handler_runs(&ns, "host.docs.get");
    // Valid by schema; the document does not exist, which is the handler's answer.
    let err = client
        .tools_call("host.docs.get", json!({"name": "nope"}))
        .await
        .expect_err("no such document");
    assert_ne!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert_eq!(handler_runs(&ns, "host.docs.get"), before + 1);
}
