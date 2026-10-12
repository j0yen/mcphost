//! PRD-mcphost-fields-projection-on-list-verbs
//! AC6 (P0) -- Given `fields: []`, When called, Then `args_invalid`; Given
//! `fields` on a verb with `unpaged_reason`, Then `args_invalid` naming the
//! argument.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::paged;
use serde_json::json;

#[tokio::test]
async fn empty_fields_and_fields_on_an_unpaged_verb_are_args_invalid_naming_fields() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "Fldp AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    for bad in [json!([]), json!("name"), json!([1])] {
        let err = client
            .tools_call("host.docs.list", json!({"fields": bad}))
            .await
            .expect_err("a malformed fields is refused");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{bad}: {err:?}");
        assert!(err.message.contains("fields"), "{bad}: {err:?}");
    }

    // host.secret.list carries an `unpaged_reason` and does not project.
    assert!(paged::unpaged_reason("host.secret.list").is_some());
    assert!(paged::fields_for("host.secret.list").is_none());
    let err = client
        .tools_call("host.secret.list", json!({"fields": ["name"]}))
        .await
        .expect_err("fields on an unpaged verb is refused");
    assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{err:?}");
    assert!(err.message.contains("fields"), "the argument is named: {err:?}");

    // and without fields the same verb still answers.
    client.tools_call("host.secret.list", json!({})).await.expect("plain call unaffected");
}
