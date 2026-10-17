//! PRD-mcphost-sandbox-file-processing-libs
//! AC6 (P0) -- Given a tool with `requirements: ["pypdf", "httpx"]`, When
//! published, Then it builds its own env (not the base) and still succeeds,
//! and the base env directory is unchanged.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::kinds::python::PythonKind;
use mcphost::sandbox;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Every path under `dir` with its size and mtime: equal before and after
/// means the directory was not touched.
fn snapshot(dir: &Path) -> BTreeMap<String, (u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read_dir") {
            let entry = entry.expect("entry");
            let meta = std::fs::symlink_metadata(entry.path()).expect("metadata");
            out.insert(
                entry.path().display().to_string(),
                (meta.len(), meta.modified().expect("mtime")),
            );
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    out
}

#[tokio::test]
async fn sblibs_ac06_requirements_beyond_the_base_build_their_own_env_and_leave_the_base_alone() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    // The base env, built the way the daemon does at start.
    let base_kind = PythonKind::new(&envs_dir.0);
    base_kind.ensure_base_env().await;
    let base_dir = base_kind
        .wait_base_env(Duration::from_secs(120))
        .await
        .expect("base env ready");
    // Let the start-time probe finish so it cannot race the snapshot.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let before = snapshot(&base_dir);

    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Sblibs AC6 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({
        "source": "import pypdf, httpx\ndef main(args):\n    return {'ok': hasattr(httpx, 'get') and hasattr(pypdf, 'PdfReader'), 'httpx_file': httpx.__file__}\n",
        "requirements": ["pypdf", "httpx"],
        "network": "none",
        "args_schema": {"type": "object"},
    });
    let published = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "own_env", "kind": "python", "spec": spec}))
            .await
            .expect("publish ok"),
    );
    assert_ne!(published["env"], json!("preinstalled"), "extra requirement is not the base: {published}");

    let mut last = json!(null);
    for _ in 0..400 {
        let result = client
            .tools_call(&format!("{ns}.own_env"), json!({}))
            .await
            .unwrap_or_else(|e| panic!("call failed: {} {}", e.code, e.message));
        last = extract_structured(&result);
        if last["status"] != json!("building") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(last["ok"], json!(true), "call result: {last}");
    let httpx_file = last["httpx_file"].as_str().expect("httpx_file");
    assert!(
        !httpx_file.starts_with(base_dir.to_str().expect("utf8 path")),
        "httpx must come from the tool's own env, not the base: {httpx_file}"
    );
    assert!(httpx_file.contains("/envs/"), "httpx lives in an env dir: {httpx_file}");

    assert_eq!(before, snapshot(&base_dir), "the base env directory must be unchanged");
}
