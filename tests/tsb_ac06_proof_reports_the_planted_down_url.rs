//! PRD-mcphost-timeseries-buckets
//! AC6 -- Given `proof.sh` rewritten on R6, When it runs against a local host,
//! Then `status` reports the planted down URL from `host.ts.get` alone and
//! `grep -cE 'state\.(query|insert)' examples/uptime-probes/tools/status.py`
//! is 0.

use crate::common::{TempDataDir, TestServer, python_kind_registry};
use mcphost::sandbox;
use tokio::process::Command;

fn example_path(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/uptime-probes").join(rel)
}

#[tokio::test]
async fn tsb_ac06_status_source_never_reads_or_writes_a_state_table() {
    // The grep the PRD names, run the way the PRD names it (exit 1 = no match).
    let out = Command::new("grep")
        .args(["-cE", r"state\.(query|insert)"])
        .arg(example_path("tools/status.py"))
        .output()
        .await
        .expect("run grep");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0", "status.py must not touch state.query/state.insert");
    let status = std::fs::read_to_string(example_path("tools/status.py")).expect("status.py");
    assert!(status.contains("mcphost.ts.get("), "status.py reads host.ts.get through the bridge");
    let probe = std::fs::read_to_string(example_path("tools/probe.py")).expect("probe.py");
    for needle in [r#"UP_SERIES = "up""#, r#"LATENCY_SERIES = "latency_ms""#, "mcphost.ts.put("] {
        assert!(probe.contains(needle), "probe.py must contain {needle}");
    }
}

#[tokio::test]
async fn tsb_ac06_proof_sh_reports_the_planted_down_url_from_host_ts_get() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let out = Command::new("bash")
        .arg(example_path("proof.sh"))
        .env("MCPHOST_URL", format!("{}/mcp", server.base_url))
        .env_remove("PROBE_NETWORK")
        .output()
        .await
        .expect("run uptime-probes proof.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    println!("{stdout}");
    assert!(out.status.success(), "proof.sh must pass against a local host:\n{stdout}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.last().copied(), Some("status: down=1 up=2 via host.ts.get"), "the last line:\n{stdout}");
    for check in [
        "CHECK status_source_has_no_state_query: PASS",
        "CHECK planted_down_url_is_down: PASS",
        "CHECK down_url_has_down_since: PASS",
        "CHECK status_reports_down_1_up_2: PASS",
    ] {
        assert!(lines.contains(&check), "missing `{check}`:\n{stdout}");
    }
    assert!(!lines.iter().any(|l| l.contains(": FAIL")), "no check may fail:\n{stdout}");
}
