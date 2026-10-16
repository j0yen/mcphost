//! PRD-mcphost-migration-refusal-contract-b AC3 (P1) -- Given a report with
//! `ok: false` in the data dir newer than process start, When `GET
//! /status.json`, Then `deploy.last_refusal.step` is set; Given none, Then
//! `deploy.last_refusal` is `null`.

use crate::common;

use common::{TempDataDir, TestServer, python_kind_registry};

async fn status(base: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base}/status.json"))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse /status.json")
}

#[tokio::test]
async fn mrefub_ac03_last_refusal_follows_the_report_in_the_data_dir() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let data_dir = server.state.db.data_dir().to_path_buf();
    let report = data_dir.join(mcphost::statusfeed::COMPAT_REPORT_FILE);

    let body = status(&server.base_url).await;
    assert!(body["deploy"]["last_refusal"].is_null(), "no report: {body}");

    // ok:true is not a refusal.
    std::fs::write(&report, r#"{"ok": true, "from": "0.106.0", "to": "0.106.0"}"#).unwrap();
    let body = status(&server.base_url).await;
    assert!(body["deploy"]["last_refusal"].is_null(), "ok:true report: {body}");

    std::fs::write(
        &report,
        r#"{"ok": false, "from": "0.106.0", "to": "0.107.0", "step": "tools_call", "checked_at_unix": 1760000000, "detail": "secret detail"}"#,
    )
    .unwrap();
    let body = status(&server.base_url).await;
    let refusal = &body["deploy"]["last_refusal"];
    assert_eq!(refusal["step"], "tools_call", "body: {body}");
    assert_eq!(refusal["from"], "0.106.0");
    assert_eq!(refusal["to"], "0.107.0");
    assert_eq!(refusal["at"], 1760000000);
    assert!(refusal.get("detail").is_none(), "public feed carries no detail text");

    // A report older than process start is stale.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400 * 365);
    std::fs::File::options().write(true).open(&report).unwrap().set_modified(old).unwrap();
    let body = status(&server.base_url).await;
    assert!(body["deploy"]["last_refusal"].is_null(), "stale report: {body}");
}
