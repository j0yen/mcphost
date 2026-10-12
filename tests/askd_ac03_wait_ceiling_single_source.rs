//! PRD-mcphost-ask-user-decision
//! AC3 -- Given `WAIT_MAX_S`, When the `host.msg.wait` descriptor, the
//! `host.ask.wait` descriptor and `host.quickstart.limits.wait_max_s` are
//! rendered, Then all three carry the constant's value and
//! `rg 'clamp\(1, 25\)' src` returns no lines.

use crate::common;

use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::asks::WAIT_MAX_S;
use serde_json::json;
use std::path::Path;

fn description_of(tools: &serde_json::Value, name: &str) -> String {
    tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == json!(name))
        .unwrap_or_else(|| panic!("{name} must be listed"))["description"]
        .as_str()
        .expect("description")
        .to_string()
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[tokio::test]
async fn descriptors_and_quickstart_limits_carry_the_constant_and_no_literal_clamp_remains() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "askd ac03 tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let tools = client.tools_list().await.expect("tools/list");
    let ceiling = format!("max {WAIT_MAX_S}s");
    for name in ["host.msg.wait", "host.ask.wait"] {
        let description = description_of(&tools, name).replace("\\\n", " ");
        let squeezed = description.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(squeezed.contains(&ceiling), "{name} descriptor must say '{ceiling}': {squeezed}");
    }

    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert_eq!(quickstart["limits"]["wait_max_s"], json!(WAIT_MAX_S), "{quickstart}");

    // The clamps themselves: no second copy of the literal anywhere in src.
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let offenders: Vec<String> = files
        .iter()
        .filter(|f| std::fs::read_to_string(f).is_ok_and(|s| s.contains("clamp(1, 25)")))
        .map(|f| f.display().to_string())
        .collect();
    assert!(offenders.is_empty(), "clamp(1, 25) literals must be gone: {offenders:?}");
}
