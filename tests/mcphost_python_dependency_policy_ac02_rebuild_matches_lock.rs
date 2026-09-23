//! PRD-mcphost-python-dependency-policy
//! AC2 (P0) — Given a stored lock, When the env is rebuilt a day later with
//! a newer upstream release available, Then the resolved versions equal
//! the lock.
//!
//! No real day-long wait or process restart is needed to prove this: a
//! lock pinned to a deliberately old `idna` release (2.10 -- long since
//! superseded on PyPI) is supplied as `requirements` (the requirement-5/
//! AC7 supplied-lock path), the env is built from it, and the tool's own
//! `idna.__version__` is asserted to be exactly that old pin. If the build
//! ever "helpfully" re-resolved from the bare name instead of installing
//! from the stored lock, it would install whatever `idna` is current today
//! -- observably not 2.10.

use crate::common;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

const OLD_IDNA_VERSION: &str = "2.10";

/// Builds a real `--generate-hashes` lock for `idna==2.10` with the real
/// `uv` this box already has (same tool `kinds::python::run_build_steps`
/// itself shells out to) -- not a hand-typed fixture, so its hashes are
/// guaranteed to match the real distribution `uv pip sync --require-hashes`
/// will later install.
async fn compile_old_idna_lock() -> String {
    let scratch = std::env::temp_dir().join(format!(
        "mcphost-ac02-fixture-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    tokio::fs::create_dir_all(&scratch).await.expect("scratch dir");
    let reqs_path = scratch.join("requirements.in");
    let lock_path = scratch.join("lock.txt");
    tokio::fs::write(&reqs_path, format!("idna=={OLD_IDNA_VERSION}\n"))
        .await
        .expect("write requirements.in");
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
    assert!(
        output.status.success(),
        "uv pip compile idna=={OLD_IDNA_VERSION} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = tokio::fs::read_to_string(&lock_path).await.expect("read lock.txt");
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    text
}

#[tokio::test]
async fn rebuilt_env_installs_exactly_the_locked_old_version() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let lock_text = compile_old_idna_lock().await;
    assert!(
        lock_text.contains(&format!("idna=={OLD_IDNA_VERSION}")),
        "fixture lock must actually pin the old version: {lock_text}"
    );

    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC2 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    // requirement 5 (AC7): the whole multi-line `--require-hashes` lock,
    // supplied as a single `requirements` entry.
    let spec = json!({
        "source": "import idna\ndef main(args):\n    return {\"version\": idna.__version__}\n",
        "requirements": [lock_text],
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "oldidna", "kind": "python", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let mut last = json!(null);
    for _ in 0..200 {
        let result = client
            .tools_call(&format!("{ns}.oldidna"), json!({}))
            .await
            .unwrap_or_else(|e| panic!("call failed: {} {}", e.code, e.message));
        let structured = extract_structured(&result);
        if structured["status"] != json!("building") {
            last = structured;
            break;
        }
        last = structured;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    assert_eq!(
        last["version"],
        json!(OLD_IDNA_VERSION),
        "the env must be built from the stored lock, installing exactly the pinned \
         old version regardless of whatever idna release is current upstream today; got {last:?}"
    );
}
