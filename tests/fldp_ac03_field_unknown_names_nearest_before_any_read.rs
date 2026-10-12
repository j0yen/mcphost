//! PRD-mcphost-fields-projection-on-list-verbs
//! AC3 (P0) -- Given `fields: ["nmae"]` on `host.docs.list`, When called,
//! Then `field_unknown` with `data.nearest == "name"` and `data.valid` equal
//! to the contract's `fields_enum` for that verb, and no DB read occurred.
//!
//! The server has no query log, so "no read" is proved structurally: the
//! `documents` table is renamed away under the running server, after which a
//! valid `docs.list` fails on its read while the bad-field call still answers
//! `field_unknown` -- it was refused before the table was touched.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::{Value, json};

fn contract_fields_enum(verb: &str) -> Vec<Value> {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json"))
        .expect("committed contract");
    let contract: Value = serde_json::from_str(&raw).expect("contract json");
    contract["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == json!(verb))
        .unwrap_or_else(|| panic!("{verb} not in contract"))["fields_enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{verb} has no fields_enum"))
        .clone()
}

#[tokio::test]
async fn a_typo_in_fields_names_the_nearest_field_and_the_valid_set_without_reading() {
    let server = TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "Fldp AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.docs.put", json!({"name": "alpha", "content": "x"}))
        .await
        .expect("put");

    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).expect("raw db");
    conn.execute_batch("ALTER TABLE documents RENAME TO documents_gone").expect("rename documents away");

    // Control: a valid call now fails, so any call that reached the table would too.
    let read = client.tools_call("host.docs.list", json!({"fields": ["name"]})).await;
    assert!(read.is_err(), "the read path must be broken for this proof to mean anything: {read:?}");
    assert_ne!(read.unwrap_err().error_code.as_deref(), Some("field_unknown"));

    let err = client
        .tools_call("host.docs.list", json!({"fields": ["nmae"]}))
        .await
        .expect_err("an unknown field is refused");
    assert_eq!(err.error_code.as_deref(), Some("field_unknown"), "{err:?}");
    assert_eq!(err.message, "unknown field 'nmae' for host.docs.list", "{err:?}");
    assert_eq!(err.data["field"], json!("nmae"), "{err:?}");
    assert_eq!(err.data["nearest"], json!("name"), "{err:?}");
    assert_eq!(err.data["valid"], json!(contract_fields_enum("host.docs.list")), "{err:?}");

    let far = client
        .tools_call("host.docs.list", json!({"fields": ["zzzzzzzzzz"]}))
        .await
        .expect_err("far from everything");
    assert_eq!(far.error_code.as_deref(), Some("field_unknown"), "{far:?}");
    assert_eq!(far.data["nearest"], Value::Null, "{far:?}");
}
