//! AC5 (P0) — Given the recorded assignment-expression spec, When
//! validated, Then the rejection message names assignment expressions and
//! the accepted alternative in one sentence.
//!
//! Fixture note: the PRD quotes the rejection as "cannot use assignment
//! expressions with expression" at line 27, but CPython's own SyntaxError
//! for an invalid assignment-expression *target* (the only class of
//! assignment-expression restriction CPython's grammar actually enforces)
//! names the target kind precisely -- verified interactively:
//! `(obj.attr := 1)` raises `cannot use assignment expressions with
//! attribute`; `(d[k] := 1)` raises `...with subscript`. This fixture
//! reproduces that real, checkable failure at line 27 rather than the
//! PRD's paraphrase, and asserts the enhanced message names both the
//! construct and the accepted alternative in one sentence.

mod common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn assignment_expression_target_rejection_names_construct_and_alternative() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // Filler lines so the offending statement lands on line 27, exactly as
    // the PRD's fixture describes (line 1 is `def main`, 24 filler lines
    // are 2..25, `obj = args` is 26, the walrus misuse is 27).
    let mut source = String::from("def main(args):\n");
    for i in 0..24 {
        source.push_str(&format!("    _unused_{i} = {i}\n"));
    }
    source.push_str("    obj = args\n"); // line 26
    source.push_str("    result = (obj.attr := 1)\n"); // line 27
    source.push_str("    return {'result': result}\n");

    let spec = json!({"source": source, "args_schema": {"type": "object"}});
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_walrus", "kind": "python", "spec": spec}),
        )
        .await
        .expect_err("an assignment expression targeting an attribute is invalid Python");

    assert!(
        err.message.contains("line 27"),
        "must name line 27, got: {}",
        err.message
    );
    assert!(
        err.message.contains("assignment expression"),
        "must name the offending construct, got: {}",
        err.message
    );
    // The accepted alternative, in the same sentence -- not a second
    // error, not a separate field.
    assert!(
        err.message.contains("separate")
            && (err.message.contains("statement") || err.message.contains("assign")),
        "must name the accepted alternative in the same sentence, got: {}",
        err.message
    );
}
