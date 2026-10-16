//! PRD-mcphost-host-doctor-next-names-envelope-key
//! AC2 -- indexing a bridge envelope with a key it lacks is a `tool_exception`
//! `KeyError` whose message and data name the bridge function and its keys; a
//! `KeyError` on the tool's own dict carries neither.

use crate::common;
use common::{TestServer, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

const SETUP: &str = "import mcphost\ndef main(args):\n    mcphost.state.table_create('t', {'name': 'text'})\n    return {'ok': True}\n";
const ENVELOPE: &str =
    "import mcphost\ndef main(args):\n    env = mcphost.state.query(table='t'); return {'first': env[0]['name']}\n";
const OWN_DICT: &str = "def main(args):\n    own = {'a': 1}\n    return {'x': own['b']}\n";

#[tokio::test]
async fn hdnext_ac02() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Hdnext AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    for (name, source) in [("setup", SETUP), ("envidx", ENVELOPE), ("owndict", OWN_DICT)] {
        client
            .tools_call("host.tool_publish", json!({"name": name, "kind": "python", "spec": {"source": source}}))
            .await
            .expect("publish ok");
    }
    poll_until_ready(&client, &format!("{ns}.setup"), json!({}), Duration::from_secs(30))
        .await
        .unwrap_or_else(|e| panic!("setup must run: {} {}", e.code, e.message));

    let e = poll_until_ready(&client, &format!("{ns}.envidx"), json!({}), Duration::from_secs(30))
        .await
        .expect_err("indexing the envelope with 0 must fail");
    assert_eq!(e.error_code.as_deref(), Some("tool_exception"), "{} {}", e.message, e.data);
    assert!(e.message.starts_with("KeyError: "), "{}", e.message);
    // The live `mcphost.state.query` envelope is `{rows, table}`: the clause
    // carries the object's own keys, whatever the table documents.
    assert!(
        e.message.contains(r#"-- mcphost.state.query returns a dict with keys ["rows", "table"]"#),
        "{}",
        e.message
    );
    assert_eq!(e.data["exception_class"], json!("KeyError"), "{}", e.data);
    assert_eq!(e.data["envelope_fn"], json!("mcphost.state.query"), "{}", e.data);
    assert_eq!(e.data["envelope_keys"], json!(["rows", "table"]), "{}", e.data);
    assert_eq!(e.data["line"], json!(3), "{}", e.data);

    let e = poll_until_ready(&client, &format!("{ns}.owndict"), json!({}), Duration::from_secs(30))
        .await
        .expect_err("own dict KeyError");
    assert_eq!(e.data["exception_class"], json!("KeyError"), "{}", e.data);
    let obj = e.data.as_object().expect("data object");
    assert!(!obj.contains_key("envelope_fn") && !obj.contains_key("envelope_keys"), "{}", e.data);
}
