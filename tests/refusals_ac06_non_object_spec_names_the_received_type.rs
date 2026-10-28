//! PRD-mcphost-refusals-name-their-cause AC6 -- Given the registered kinds
//! (`kinds::names()`) and chain `spec.steps[i]`, When the spec or a step is
//! not a JSON object, Then the `-32602` message keeps each site's current
//! prefix and ends with `, received <string|array|number|boolean|null>`,
//! `data.received_type` carries the same word, the object check runs once in
//! the registry before any kind's `validate`, and one test iterates
//! `kinds::names()` so a kind added later cannot skip it.

use crate::common;

use common::{McpClient, TestServer};
use mcphost::errors::AppError;
use mcphost::kinds::{KindError, KindRegistry};
use serde_json::{Value, json};

const CASES: [(&str, fn() -> Value); 5] = [
    ("string", || json!("a string")),
    ("array", || json!([1, 2])),
    ("number", || json!(7)),
    ("boolean", || json!(true)),
    ("null", || Value::Null),
];

/// The wire `-32602` an `AppError` becomes: `(message, data)`.
fn wire(err: KindError) -> (String, Value) {
    let data = AppError::from(err).into_error_data();
    assert_eq!(data.code.0, -32602, "{data:?}");
    (data.message.to_string(), data.data.expect("data"))
}

fn assert_refusal(message: &str, data: &Value, prefix: &str, word: &str) {
    assert!(message.contains(prefix), "prefix {prefix:?} kept: {message}");
    assert!(message.ends_with(&format!(", received {word}")), "suffix: {message}");
    assert_eq!(data["received_type"], word, "{data}");
    assert_eq!(data["error_code"], "invalid_spec");
}

#[test]
fn every_registered_kind_and_the_registry_refuse_a_non_object_spec_with_its_type() {
    let kinds = mcphost::kinds::production_registry();
    let names = kinds.names();
    for expected in ["chain", "echo", "http", "jq", "js", "python", "wasm"] {
        assert!(names.contains(&expected), "the production registry lists {expected}: {names:?}");
    }
    for name in names {
        let kind = kinds.get(name).unwrap_or_else(|| panic!("kind {name}"));
        for (word, make) in CASES {
            let spec = make();
            // The registry check runs first and decides alone: its answer is the shape error
            // whatever the kind would say.
            let checked = KindRegistry::validate_all_checked(kind.as_ref(), &spec);
            assert_eq!(checked.len(), 1, "{name} {word}: {checked:?}");
            let (message, data) = wire(checked.into_iter().next().unwrap());
            assert_eq!(message, format!("invalid spec: spec: must be a JSON object, received {word}"), "{name}");
            assert_refusal(&message, &data, "spec: must be a JSON object", word);
            let single = KindRegistry::validate_checked(kind.as_ref(), &spec).expect_err("refused");
            assert_eq!(wire(single).0, message, "{name}: validate_checked answers like validate_all_checked");

            // The kind's own check stays as a backstop with the same prefix, suffix and field.
            let own = kind.validate_all(&spec);
            assert_eq!(own.len(), 1, "{name} {word} own validate: {own:?}");
            let (own_message, own_data) = wire(own.into_iter().next().unwrap());
            assert_refusal(&own_message, &own_data, "spec: must be a JSON object", word);
            assert_eq!(own_message, message, "{name}: the backstop and the registry agree");
        }
        // An object spec is not this check's business.
        assert!(KindRegistry::check_spec_shape(&json!({})).is_ok());
    }
}

#[test]
fn a_chain_step_that_is_not_an_object_names_its_received_type() {
    let kinds = mcphost::kinds::production_registry();
    let chain = kinds.get("chain").expect("chain kind");
    for (word, make) in CASES {
        let spec = json!({"steps": [make()]});
        let errors = chain.validate_all(&spec);
        assert_eq!(errors.len(), 1, "{word}: {errors:?}");
        let (message, data) = wire(errors.into_iter().next().unwrap());
        assert_eq!(message, format!("invalid spec: spec.steps[0]: must be a JSON object, received {word}"));
        assert_refusal(&message, &data, "spec.steps[0]: must be a JSON object", word);
    }
    // The second step's index is named too, and a bad `args` keeps its longer prefix.
    let spec = json!({"steps": [{"tool": "a.b"}, 5]});
    let (message, data) = wire(chain.validate_all(&spec).into_iter().next().expect("refused"));
    assert_refusal(&message, &data, "spec.steps[1]: must be a JSON object", "number");
    let spec = json!({"steps": [{"tool": "a.b", "args": 3}]});
    let (message, data) = wire(chain.validate_all(&spec).into_iter().next().expect("refused"));
    assert_refusal(&message, &data, "spec.steps[0].args: must be a JSON object, or the string \"$.prev\"", "number");
}

#[test]
fn the_three_shared_sites_go_through_the_registry_check() {
    for (file, text) in [
        ("src/control.rs", include_str!("../src/control.rs")),
        ("src/handler.rs", include_str!("../src/handler.rs")),
    ] {
        assert!(text.contains("KindRegistry::validate_all_checked(") || text.contains("KindRegistry::validate_checked("), "{file}");
    }
    let calls = |text: &str, needle: &str| text.matches(needle).count();
    let control = include_str!("../src/control.rs");
    let handler = include_str!("../src/handler.rs");
    assert_eq!(calls(control, "KindRegistry::validate_all_checked("), 1);
    assert_eq!(calls(handler, "KindRegistry::validate_all_checked("), 1);
    assert_eq!(calls(handler, "KindRegistry::validate_checked("), 1);
    assert_eq!(calls(control, "kind.validate_all(&spec)"), 0, "no shared site validates around the registry");
    assert_eq!(calls(handler, "kind.validate_all(&spec)"), 0);
}

#[tokio::test]
async fn host_tool_publish_over_http_names_the_received_type() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "refusals ac6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for name in server.state.kinds.names() {
        for (word, make) in CASES {
            let err = client
                .tools_call("host.tool_publish", json!({"name": "t_ac6", "kind": name, "spec": make()}))
                .await
                .expect_err("a non-object spec is refused");
            assert_eq!(err.code, -32602, "{name} {word}: {err:?}");
            assert_eq!(
                err.message,
                format!("invalid spec: spec: must be a JSON object, received {word}"),
                "{name} {word}: {err:?}"
            );
            assert_eq!(err.data["received_type"], word, "{name} {word}: {:?}", err.data);
            assert_eq!(err.error_code.as_deref(), Some("invalid_spec"));
        }
    }
}
