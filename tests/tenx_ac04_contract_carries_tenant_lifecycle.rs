//! PRD-mcphost-synthetic-implicit-tenant-expiry
//! AC4 (P0) — Given `mcphost contract dump` in a clean checkout and a server
//! of that build, When both are read, Then `contracts/host-tools.v1.json`
//! carries a top-level `tenant_lifecycle` (selector, env var, default,
//! reason) built by `api_contract` from the policy constants,
//! `contract_version` follows the file's rule, `GET /contract.json` returns the
//! binary's bytes, and `contract --check` fails when a test-only override
//! moves the default.

use crate::common::tenx;
use crate::common::{EnvGuard, TestServer};
use mcphost::api_contract::{CONTRACT_VERSION, ContractSnapshot, dump_contract, dump_contract_bytes, tenant_lifecycle};
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::process::Command;

const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

fn check(path: &str, override_days: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcphost"));
    cmd.args(["contract", "dump", "--check", "--path", path]);
    if let Some(days) = override_days {
        cmd.env("MCPHOST_TEST_ONLY_TENANT_LIFECYCLE_DEFAULT_DAYS", days);
    }
    cmd.output().expect("run mcphost contract dump --check")
}

#[tokio::test]
async fn tenx_ac04_committed_dump_and_served_contract_carry_the_lifecycle_rule() {
    let _env = EnvGuard::clear(tenx::ENV).await;
    let committed: Value = serde_json::from_slice(&std::fs::read(COMMITTED).unwrap()).unwrap();
    let rule = &committed["tenant_lifecycle"];
    assert_eq!(rule, &tenant_lifecycle(), "the committed key is the one api_contract builds");
    assert_eq!(rule["selector"]["origin"], "synthetic");
    assert_eq!(rule["selector"]["signup_source"], "implicit");
    assert_eq!(rule["env_var"], "MCPHOST_RETENTION_SYNTHETIC_IMPLICIT_TENANT_IDLE_DAYS");
    assert_eq!(rule["default_days"], 1);
    assert_eq!(rule["disabled_reason"], "idle_expired");
    // An additive top-level key leaves the file's own integer alone (it only moves for a breaking shape).
    assert_eq!(committed["contract_version"], CONTRACT_VERSION);

    // The committed file is exactly what the binary dumps.
    let kinds = KindRegistry::with_builtin();
    assert_eq!(std::fs::read(COMMITTED).unwrap(), dump_contract_bytes(&kinds), "committed == dump");
    assert_eq!(dump_contract(&kinds)["tenant_lifecycle"], *rule);

    // A server of this build answers with the binary's bytes.
    let server = TestServer::start().await;
    let body = reqwest::Client::new()
        .get(format!("{}/contract.json", server.base_url))
        .send()
        .await
        .expect("GET /contract.json")
        .bytes()
        .await
        .expect("body");
    let served: Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(served["tenant_lifecycle"], *rule, "served rule equals the dump's");
    assert_eq!(body.as_ref(), ContractSnapshot::builtin().served.as_slice(), "served bytes are the binary's");
    let mut without_sha = served.clone();
    without_sha.as_object_mut().expect("object").remove("contract_sha");
    assert_eq!(without_sha, dump_contract(&kinds), "served doc is the dump plus contract_sha");
}

#[tokio::test]
async fn tenx_ac04_check_passes_clean_and_fails_when_the_default_moves() {
    // Holds the process-wide env lock so no sibling test's variable reaches the child.
    let _env = EnvGuard::clear(tenx::ENV).await;
    let clean = check(COMMITTED, None);
    assert!(clean.status.success(), "clean checkout passes: {}", String::from_utf8_lossy(&clean.stderr));
    let moved = check(COMMITTED, Some("7"));
    assert!(!moved.status.success(), "a moved default must fail --check");
    let text = format!("{}{}", String::from_utf8_lossy(&moved.stdout), String::from_utf8_lossy(&moved.stderr));
    assert!(text.contains("stale"), "check says the file is stale: {text}");
}
