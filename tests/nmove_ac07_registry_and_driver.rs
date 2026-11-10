//! PRD-mcphost-refusals-name-the-nearest-move
//! AC7 (P0) -- Given the regenerated `error_registry` output `errors.json`,
//! When its drift test runs, Then `tool_not_found` lists `nearest`,
//! `did_you_mean`, `tools_published` and `catalog_matches`, `invalid_spec`
//! lists `defs_found` and `fix`, `tenant_key_invalid` lists `hint`, no error
//! code is added, and an `nmove` Rust test runs `python3 -I
//! tests/live/nmove_ac08_nearest_move.py <in-process /mcp URL>` to exit 0
//! with that driver committed in the same change.

use crate::common;
use common::{TempDataDir, TestServer, python_kind_registry};
use mcphost::error_registry::{ERROR_CODES, catalog_json};
use mcphost::errors::all_variants_for_test;
use mcphost::sandbox;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

const DRIVER: &str = "tests/live/nmove_ac08_nearest_move.py";

fn read(rel: &str) -> String {
    std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn keys_of(catalog: &Value, code: &str) -> Vec<String> {
    let row = catalog["codes"].as_array().unwrap().iter().find(|r| r["code"] == code).unwrap_or_else(|| panic!("{code} is a row"));
    row["data_keys"].as_array().unwrap_or_else(|| panic!("{code} lists data_keys")).iter().map(|k| k.as_str().unwrap().to_string()).collect()
}

#[test]
fn nmove_ac07_registry_lists_the_new_keys_and_the_generated_surfaces_follow() {
    let catalog = catalog_json("http://localhost");
    let tnf = keys_of(&catalog, "tool_not_found");
    for k in ["nearest", "did_you_mean", "tools_published", "catalog_matches"] {
        assert!(tnf.iter().any(|x| x == k), "tool_not_found lists {k}: {tnf:?}");
    }
    let spec = keys_of(&catalog, "invalid_spec");
    for k in ["defs_found", "fix"] {
        assert!(spec.iter().any(|x| x == k), "invalid_spec lists {k}: {spec:?}");
    }
    assert!(keys_of(&catalog, "tenant_key_invalid").iter().any(|x| x == "hint"));

    // No code is added: the variants this PRD adds report codes that already had rows.
    let codes: Vec<&str> = ERROR_CODES.iter().map(|e| e.code).collect();
    for err in all_variants_for_test() {
        assert!(codes.contains(&err.code()), "{} is a registered code", err.code());
    }
    assert_eq!(mcphost::errors::AppError::TenantKeyIsHandoffToken.code(), "tenant_key_invalid");
    for fresh in ["tool_not_found_near", "no_main", "handoff_token_as_key", "nearest_move"] {
        assert!(!codes.contains(&fresh), "{fresh} must not be an error code");
    }

    // The checked-in generated surfaces are the generator's output and carry the keys.
    let errors_md = read("docs/errors.md");
    assert_eq!(errors_md, mcphost::retry_policy::render_doc(), "docs/errors.md is stale");
    let py = read("sdk/python/mcphost_sdk/errors.py");
    let ts = read("sdk/ts/src/errors.ts");
    for key in ["nearest", "did_you_mean", "tools_published", "catalog_matches", "defs_found", "hint"] {
        assert!(errors_md.contains(&format!("`{key}`")), "docs/errors.md lacks {key}");
        assert!(py.contains(&format!("\"{key}\"")), "python SDK lacks {key}");
        assert!(ts.contains(&format!("\"{key}\"")), "ts SDK lacks {key}");
    }
    let contract = read("contracts/host-tools.v1.json");
    assert_eq!(mcphost::sdk_python::render(contract.as_bytes()).expect("python sdk")["errors.py"], py, "python errors.py is stale");
    assert_eq!(mcphost::r#gen::sdk_ts::render(contract.as_bytes()).expect("ts sdk")["errors.ts"], ts, "ts errors.ts is stale");
}

#[tokio::test]
async fn nmove_ac07() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let url = format!("{}/mcp", server.base_url);
    let driver = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DRIVER);
    assert!(driver.is_file(), "the driver is a committed file: {}", driver.display());

    // The driver blocks on HTTP to this very server, so it runs off the runtime thread.
    let out = tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new("python3");
        cmd.args(["-I", "-B"]).arg(driver).arg(url).env("NO_PROXY", "127.0.0.1").env("no_proxy", "127.0.0.1");
        common::strip_cargo_injected_env(&mut cmd);
        cmd.output().expect("python3 runs")
    })
    .await
    .expect("driver task joins");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stdout.lines().last(), Some("nmove_ac08 tenant=offboarded rc=0"), "stdout: {stdout}; stderr: {stderr}");
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    assert!(!stdout.contains("tenant_key") && !stderr.contains("tenant_key"), "no key material is printed");

    let tenants = server.state.db.list_tenants().await.expect("tenants");
    assert_eq!(tenants.len(), 1, "the driver minted exactly one tenant");
    assert!(tenants[0].disabled, "{} is offboarded", tenants[0].namespace);
    assert_eq!(tenants[0].disabled_reason.as_deref(), Some("self_offboard"));
    assert!(!stdout.contains(&tenants[0].namespace) && !stderr.contains(&tenants[0].namespace), "no namespace is printed");
}
