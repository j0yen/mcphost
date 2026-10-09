//! PRD-mcphost-sandbox-return-shape-contract
//! AC7 (P1) -- Given `handle=True` query paths, When the parity test runs,
//! Then their shapes are in the table and match the real return.

use crate::common;
use crate::retshape_ac01_return_shape_table_parity::probe_real_returns;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::kinds::python::{BRIDGE_RETURNS, ReturnShape, runner_script_module_prelude};
use mcphost::sandbox;
use serde_json::json;
use std::collections::BTreeSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

const HANDLE_ROW: &str = "mcphost.table.query(handle=True)";

fn handle_keys() -> BTreeSet<String> {
    let row = BRIDGE_RETURNS
        .iter()
        .find(|r| r.function == HANDLE_ROW)
        .unwrap_or_else(|| panic!("{HANDLE_ROW} must be in BRIDGE_RETURNS"));
    match row.shape {
        ReturnShape::Envelope(keys) => keys.iter().map(|(k, _)| k.to_string()).collect(),
        ReturnShape::Plain(_) => panic!("a handle result is an object envelope"),
    }
}

#[tokio::test]
async fn the_handle_true_query_shape_is_in_the_table_and_matches_the_real_return() {
    let want = handle_keys();
    let Some(real) = probe_real_returns().await else { return };
    let got = real.get(HANDLE_ROW).unwrap_or_else(|| panic!("probe did not exercise {HANDLE_ROW}: {real:#?}"));
    let keys: BTreeSet<String> = got["keys"]
        .as_array()
        .unwrap_or_else(|| panic!("handle query did not return an object: {got}"))
        .iter()
        .map(|k| k.as_str().unwrap().to_string())
        .collect();
    assert_eq!(keys, want, "real handle=True keys differ from the table");

    // The plain `rows` shape is a different row -- the two must not be conflated.
    assert!(!want.contains("rows"), "a handle result carries no `rows`: {want:?}");
}

#[test]
fn query_help_documents_both_shapes_and_the_handle_variant_has_its_own_error() {
    let mut script = runner_script_module_prelude().to_string();
    script.push_str(
        "\nm = sys.modules['mcphost.table']\nprint('DOC ' + json.dumps(m.query.__doc__))\n\
         env = _Envelope({'handle': 'h'}, 'x')\n\
         import mcphost\n",
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn python3");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "python3 failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let doc: String =
        serde_json::from_str(stdout.lines().find_map(|l| l.strip_prefix("DOC ")).expect("DOC line")).unwrap();
    assert!(doc.contains("Returns: {\"rows\": list}"), "{doc}");
    assert!(doc.contains("Returns (handle=True): {\"handle\": str"), "{doc}");
}

#[tokio::test]
async fn iterating_a_handle_result_names_the_handle_variants_shape() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Retshape AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    // A handle materialises real storage, which a `host.tool_test` savepoint
    // cannot hold, so this runs as a real call and returns the caught error.
    let source = r#"import mcphost
def main(args):
    mcphost.table.create('t', {'n': 'integer'})
    mcphost.table.append('t', [{'n': 1}])
    h = mcphost.table.query('SELECT n FROM t', handle=True)
    try:
        [x for x in h]
    except mcphost.BridgeShapeError as e:
        return {'error': str(e), 'handle_kept': 'handle' in h}
    return {'error': None}
"#;
    client
        .tools_call("host.tool_publish", json!({"name": "hdl", "kind": "python", "spec": {"source": source}}))
        .await
        .expect("publish ok");
    let v = extract_structured(
        &poll_until_ready(&client, &format!("{ns}.hdl"), json!({}), Duration::from_secs(20))
            .await
            .unwrap_or_else(|e| panic!("tool must run: {} {}", e.code, e.message)),
    );
    let error = v["error"].as_str().unwrap_or_else(|| panic!("iterating a handle result must raise: {v}"));
    assert!(error.contains("mcphost.table.query(handle=True) returns {\"handle\": str"), "{error}");
    assert!(error.ends_with("iterate result[\"sample\"]"), "{error}");
    assert_eq!(v["handle_kept"], json!(true), "{v}");
}
