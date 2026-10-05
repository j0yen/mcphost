//! PRD-mcphost-run-budget-governor
//! AC11 (P0) — Given the five recipe proof scripts, When run under plan
//! defaults on the builder, Then none ends with `budget_exceeded`.
//!
//! Four of the five (`database-in-a-minute`, `first-run`, `team-memory`,
//! `share-a-tool`) are run for real -- `examples/<recipe>/proof.sh` as a
//! subprocess against a local `TestServer`, no `budget` argument anywhere
//! (plan defaults), asserting both the script's own exit code and that
//! neither its stdout nor stderr ever mentions `budget_exceeded`.
//!
//! The fifth, `uptime-probes`, needs a `pro`-plan tenant (its `probe` tool
//! declares `network: "public"`, plan-gated since
//! PRD-mcphost-sandbox-egress-allowlist) that `proof.sh`'s own public
//! `signup` step can't arrange for itself -- every existing in-repo runner
//! for this recipe's real workload (`mcphost_uptime_probes_ac0{2,3,...}`)
//! sidesteps that by signing up IN Rust and upgrading the plan before
//! publishing, via `tests/support/uptime_probes.rs`'s own
//! `create_tables`/`publish_probe`/`set_probe_schedule`/`fire_and_wait`
//! helpers and `grant_egress`'s local forward proxy -- so this leg drives
//! those same helpers directly (the identical call sequence `proof.sh`
//! itself makes) rather than racing a bash subprocess's own signup against
//! an admin plan upgrade. Same "prove the documented behavior, not the
//! literal file" deviation `tests/lanecov_ac02_*.rs` already documents for
//! its own AC.

use crate::common;
use crate::uptime_probes;

use common::{McpClient, TempDataDir, TestServer, extract_structured, http_kind_registry, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use tokio::process::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Same "blocking Command::output() would starve the in-process server's
/// own axum task" reasoning as `sharedcall_ac06`/`mcphost_database_in_a_minute_ac02`'s
/// own `run_proof` helpers.
async fn run_proof_sh(recipe: &str, mcphost_url: &str, extra_env: &[(&str, &str)]) -> (bool, String) {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("examples/{recipe}/proof.sh"));
    let mut cmd = Command::new("bash");
    cmd.arg(&script).env("MCPHOST_URL", mcphost_url);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let output = cmd.output().await.unwrap_or_else(|e| panic!("run examples/{recipe}/proof.sh: {e}"));
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(!stdout.is_empty(), "{recipe}: proof.sh produced no stdout; stderr:\n{stderr}");
    (output.status.success(), format!("{stdout}\n--- stderr ---\n{stderr}"))
}

fn assert_clean(recipe: &str, success: bool, transcript: &str) {
    assert!(success, "{recipe}: proof.sh exited non-zero overall:\n{transcript}");
    assert!(
        !transcript.contains("budget_exceeded"),
        "{recipe}: proof.sh's own transcript mentions budget_exceeded under plan defaults:\n{transcript}"
    );
}

#[tokio::test]
async fn database_in_a_minute_stays_under_budget() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let mcphost_url = format!("{}/mcp", server.base_url);
    let (success, transcript) = run_proof_sh("database-in-a-minute", &mcphost_url, &[]).await;
    assert_clean("database-in-a-minute", success, &transcript);
}

#[tokio::test]
async fn first_run_stays_under_budget() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let mcphost_url = format!("{}/mcp", server.base_url);
    let (success, transcript) = run_proof_sh("first-run", &mcphost_url, &[]).await;
    assert_clean("first-run", success, &transcript);
}

#[tokio::test]
async fn team_memory_stays_under_budget() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let mcphost_url = format!("{}/mcp", server.base_url);
    let (success, transcript) = run_proof_sh("team-memory", &mcphost_url, &[]).await;
    assert_clean("team-memory", success, &transcript);
}

#[tokio::test]
async fn share_a_tool_stays_under_budget() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let mcphost_url = format!("{}/mcp", server.base_url);
    let receipt_path = std::env::temp_dir().join(format!(
        "mcphost-budget-ac11-share-a-tool-receipt-{}-{}.json",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    let (success, transcript) = run_proof_sh(
        "share-a-tool",
        &mcphost_url,
        &[("SHARE_A_TOOL_RECEIPT", receipt_path.to_str().expect("utf8 path"))],
    )
    .await;
    std::fs::remove_file(&receipt_path).ok();
    assert_clean("share-a-tool", success, &transcript);
}

#[tokio::test]
async fn uptime_probes_recipe_workload_stays_under_budget() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }

    let upstream = MockServer::start().await;
    Mock::given(method("GET")).and(path("/ok")).respond_with(ResponseTemplate::new(200)).mount(&upstream).await;
    let healthy_url = format!("{}/ok", upstream.uri());

    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC11 uptime-probes-owner").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let _egress_guard = uptime_probes::grant_egress(&server, &ns).await;

    uptime_probes::create_tables(&client).await;
    client
        .tools_call(
            "host.state.insert",
            json!({"table": "targets", "rows": [{"url": healthy_url, "added_at": 1.0}]}),
        )
        .await
        .expect("insert target");
    uptime_probes::publish_probe(&client).await;
    uptime_probes::publish_status(&client).await;
    let trigger_id = uptime_probes::set_probe_schedule(&client).await;

    // The recipe's own "fire twice instead of waiting 5 minutes" shape
    // (same as `mcphost_uptime_probes_ac02`), no budget argument anywhere
    // -- plan defaults throughout.
    for _ in 0..2 {
        let done = uptime_probes::fire_and_wait(&client, &trigger_id).await;
        assert_eq!(done["status"], json!("done"), "uptime-probes: {done:?}");
        assert_ne!(
            done["error_class"],
            json!("budget_exceeded"),
            "uptime-probes: {done:?}"
        );
    }

    let listed = extract_structured(
        &client.tools_call("host.runs.list", json!({"verdict": "exceeded"})).await.expect("runs.list ok"),
    );
    assert_eq!(
        listed["runs"].as_array().expect("runs array").len(),
        0,
        "uptime-probes: no run may have crossed its budget: {listed}"
    );
}
