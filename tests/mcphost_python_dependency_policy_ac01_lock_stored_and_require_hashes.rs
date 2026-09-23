//! PRD-mcphost-python-dependency-policy
//! AC1 (P0) — Given `requirements: ["requests"]`, When published, Then a
//! lock with hashes is stored and the env is built with `--require-hashes`.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn requests_publish_stores_hashed_lock_and_builds_with_require_hashes() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import requests\ndef main(args):\n    return {\"has_get\": hasattr(requests, 'get')}\n",
        "requirements": ["requests"],
        "args_schema": {"type": "object"},
    });
    let published = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "deps", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");
    let published = extract_structured(&published);

    // requirement 1 (user story): "host.tool_publish returns lock:
    // {packages: N, hashes: true}".
    assert_eq!(published["lock"]["hashes"], json!(true), "{published:?}");
    let packages = published["lock"]["packages"]
        .as_u64()
        .expect("lock.packages must be a number");
    assert!(packages >= 1, "expected at least requests itself, got {published:?}");

    // Force the (real, sandboxed) env build.
    let result = client
        .tools_call(&format!("{ns}.deps"), json!({}))
        .await
        .unwrap_or_else(|e| panic!("call failed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    // A `building` result is acceptable (the build may not finish inside
    // the wait bound on a loaded box); either way, the env directory and
    // its lock file below are written the moment the build starts, not
    // only once it finishes.
    if structured["status"] != json!("building") {
        assert_eq!(structured["has_get"], json!(true), "{structured:?}");
    }

    // requirement 1: the env was (or is being) built from a real
    // `requirements.lock` written to disk with `--require-hashes` content
    // -- the old `uv pip install <names>` path never wrote such a file.
    let ns_dir = envs_dir.0.join("envs").join(&ns);
    let mut lock_path = None;
    for _ in 0..200 {
        if let Ok(mut entries) = std::fs::read_dir(&ns_dir)
            && let Some(entry) = entries.find_map(Result::ok)
        {
            let candidate = entry.path().join("requirements.lock");
            if candidate.is_file() {
                lock_path = Some(candidate);
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let lock_path = lock_path.unwrap_or_else(|| {
        panic!("no requirements.lock ever appeared under {ns_dir:?}")
    });
    let lock_text = std::fs::read_to_string(&lock_path).expect("read requirements.lock");
    assert!(
        lock_text.contains("requests=="),
        "lock must pin requests, got:\n{lock_text}"
    );
    assert!(
        lock_text.contains("--hash=sha256:"),
        "lock must carry real hashes, got:\n{lock_text}"
    );
}
