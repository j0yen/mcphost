//! PRD-mcphost-python-dependency-policy
//! AC7 (P1) — Given a lock text with hashes supplied as `requirements`,
//! When published, Then the host validates and stores it without
//! re-resolving.
//!
//! Proof that no re-resolution happens: the supplied lock names a package
//! (`idna`) at a deliberately old, specific pin (2.10) nothing upstream
//! would produce today; a fresh `uv pip compile` resolve of the bare name
//! `idna` would pick today's current release instead. The published
//! tool's own reported version is asserted to be exactly the supplied pin.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

#[tokio::test]
async fn hashed_lock_supplied_as_requirements_is_stored_verbatim() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let scratch = std::env::temp_dir().join(format!(
        "mcphost-ac07-fixture-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    tokio::fs::create_dir_all(&scratch).await.expect("scratch dir");
    let reqs_path = scratch.join("requirements.in");
    let lock_path = scratch.join("lock.txt");
    tokio::fs::write(&reqs_path, "idna==2.10\n").await.expect("write requirements.in");
    let output = tokio::process::Command::new("uv")
        .arg("pip")
        .arg("compile")
        .arg(&reqs_path)
        .arg("--generate-hashes")
        .arg("-o")
        .arg(&lock_path)
        .output()
        .await
        .expect("spawn uv pip compile");
    assert!(output.status.success(), "fixture compile failed: {output:?}");
    let lock_text = tokio::fs::read_to_string(&lock_path).await.expect("read lock.txt");
    let _ = tokio::fs::remove_dir_all(&scratch).await;

    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC7 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import idna\ndef main(args):\n    return {\"version\": idna.__version__}\n",
        "requirements": [lock_text],
        "args_schema": {"type": "object"},
    });
    let published = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "pinned", "kind": "python", "spec": spec}),
        )
        .await
        .expect("a valid supplied lock must publish, not fail validation");
    let published = extract_structured(&published);
    assert_eq!(published["lock"]["hashes"], json!(true), "{published:?}");
    assert_eq!(
        published["lock"]["packages"],
        json!(1),
        "exactly the one package the supplied lock named: {published:?}"
    );

    let mut last = json!(null);
    for _ in 0..200 {
        let result = client
            .tools_call(&format!("{ns}.pinned"), json!({}))
            .await
            .unwrap_or_else(|e| panic!("call failed: {} {}", e.code, e.message));
        let structured = extract_structured(&result);
        last = structured.clone();
        if structured["status"] != json!("building") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(
        last["version"],
        json!("2.10"),
        "the exact supplied pin must be what's installed -- no re-resolution to a \
         newer release; got {last:?}"
    );
}
