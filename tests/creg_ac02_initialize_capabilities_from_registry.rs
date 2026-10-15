//! AC2 (P0) -- PRD-mcphost-server-capabilities-from-registry. `initialize`
//! answers exactly the `implemented: true` rows rendered through
//! `server_capabilities()`, and no source file but `capabilities.rs` names a
//! capability by hand.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::capabilities::{CAPABILITIES, server_capabilities};
use serde_json::json;

#[tokio::test]
async fn initialize_capabilities_equal_the_implemented_rows() {
    let server = TestServer::start().await;
    let init = McpClient::new(&server.base_url).initialize().await;
    let advertised = &init["result"]["capabilities"];

    let rendered = serde_json::to_value(server_capabilities()).expect("serialize");
    assert_eq!(advertised, &rendered, "initialize.capabilities must be server_capabilities()");

    // Independent of server_capabilities(): the advertised keys are exactly
    // the implemented rows (`tools` -> "tools", ...).
    let mut expected: Vec<&str> = CAPABILITIES.iter().filter(|c| c.implemented).map(|c| c.name).collect();
    expected.sort_unstable();
    let mut actual: Vec<&str> = advertised.as_object().expect("capabilities object").keys().map(String::as_str).collect();
    actual.sort_unstable();
    assert_eq!(actual, expected, "advertised primitives vs implemented rows");
    assert_eq!(advertised["tools"], json!({"listChanged": true}));
}

fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            walk(&path, hits);
        } else if path.extension().is_some_and(|e| e == "rs")
            && path.file_name().is_some_and(|n| n != "capabilities.rs")
        {
            let text = std::fs::read_to_string(&path).expect("read");
            for (i, line) in text.lines().enumerate() {
                if line.contains("enable_tools()") {
                    hits.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
                }
            }
        }
    }
}

#[test]
fn no_source_file_but_capabilities_rs_names_a_capability_by_hand() {
    let mut hits = Vec::new();
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut hits);
    assert!(hits.is_empty(), "enable_tools() outside capabilities.rs:\n{}", hits.join("\n"));
}
