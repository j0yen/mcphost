//! PRD-mcphost-tools-call-progress-notifications
//! AC8 (P0) — Given the fixture of AC 1, When admin healthz, `mcphost funnel
//! --json` and the anonymous `/healthz` are read, Then
//! `progress_7d.calls_total`, `calls_with_token` and `frames_sent` have
//! identical key sets, `mcphost funnel --json` prints the same `progress_7d`
//! object as admin healthz, and the anonymous body still has exactly
//! `ANON_HEALTHZ_KEYS`.

use crate::common;
use crate::tcpn;
use tcpn::ac1_fixture;
use common::ADMIN_KEY;
use serde_json::Value;
use std::collections::BTreeSet;

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("an object").keys().cloned().collect()
}

async fn healthz(base_url: &str, admin: bool) -> Value {
    let req = reqwest::Client::new().get(format!("{base_url}/healthz"));
    let req = if admin { req.bearer_auth(ADMIN_KEY) } else { req };
    req.send().await.expect("GET /healthz").json().await.expect("parse /healthz")
}

#[tokio::test]
async fn tcpn_ac08_progress_7d_is_one_object_on_every_surface() {
    let server = ac1_fixture().await;
    let admin = healthz(&server.base_url, true).await;
    let p = &admin["progress_7d"];
    assert_eq!(keys(&p["calls_total"]), keys(&p["calls_with_token"]), "{p}");
    assert_eq!(keys(&p["calls_total"]), keys(&p["frames_sent"]), "{p}");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["funnel", "--json"])
        .env("MCPHOST_DATA_DIR", &server.data_dir.0)
        .output()
        .expect("spawn mcphost funnel");
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    let cli: Value = serde_json::from_slice(&output.stdout).expect("--json parses");
    assert_eq!(cli["progress_7d"], *p, "funnel --json prints the admin healthz object");

    let anon = healthz(&server.base_url, false).await;
    assert!(anon.get("progress_7d").is_none(), "{anon}");
    let want: BTreeSet<String> = mcphost::ANON_HEALTHZ_KEYS.iter().map(|k| k.to_string()).collect();
    assert_eq!(keys(&anon), want, "the anonymous body is exactly ANON_HEALTHZ_KEYS");
}
