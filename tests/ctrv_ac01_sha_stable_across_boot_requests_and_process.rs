//! PRD-mcphost-contract-version-reported AC1 — Given
//! `KindRegistry::with_builtin()`, When `contract_sha` is computed at
//! `AppState` build, again after 100 `tools/list` calls, and in a second
//! process via `mcphost contract dump --check`, Then all three shas are
//! equal; a difference fails naming the first differing byte offset.

use crate::common;
use common::{TestServer, signup};
use mcphost::api_contract::contract_sha;
use mcphost::kinds::KindRegistry;

fn assert_same(label: &str, a: &str, b: &str) {
    if a != b {
        let offset = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        panic!("{label}: shas differ at first differing byte offset {offset}: {a} vs {b}");
    }
}

#[tokio::test]
async fn contract_sha_is_equal_at_boot_after_100_tools_list_and_in_a_second_process() {
    let server = TestServer::start().await;
    let boot = server.state.contract.sha.clone();
    assert_same(
        "boot vs fresh compute",
        &boot,
        &contract_sha(&KindRegistry::with_builtin()),
    );

    let (_ns, key) = signup(&server.base_url, "CTRV AC1").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    for _ in 0..100 {
        client.tools_list().await.expect("tools/list");
    }
    assert_same(
        "boot vs after 100 tools/list",
        &boot,
        &contract_sha(&KindRegistry::with_builtin()),
    );
    assert_same("boot vs stored", &boot, &server.state.contract.sha);

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["contract", "dump", "--check"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mcphost contract dump --check");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "dump --check failed: {stdout} {}", String::from_utf8_lossy(&out.stderr));
    let second = stdout
        .split("contract_sha ")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_hexdigit()).next())
        .expect("--check prints contract_sha");
    assert_same("boot vs second process", &boot, second);
}
