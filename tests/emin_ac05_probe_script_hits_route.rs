//! PRD-mcphost-email-inbound-route
//! AC5 -- Given the server started in-process, When `scripts/email-probe.sh
//! attach lines.csv` runs as a subprocess with `MCPHOST_URL`,
//! `MCPHOST_NAMESPACE` and the secret exported, Then it exits 0 and its
//! stdout contains `blob_id=blob_` and `bytes=4096`.

use crate::common::{EnvGuard, TempDataDir, TestServer, signup};

#[tokio::test]
async fn emin_ac05_probe_script_delivers_a_csv_through_the_route() {
    let secret = "emin-ac5-secret";
    let _secret = EnvGuard::set("MCPHOST_EMAIL_INBOUND_SECRET", secret).await;
    // Empty fleet list: the probe is a non-fleet caller and must use the header.
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Emin AC5").await;

    let dir = TempDataDir::new();
    let mut csv = String::from("a,b\n");
    while csv.len() < 4096 {
        csv.push_str("1,2\n");
    }
    csv.truncate(4096);
    std::fs::write(dir.0.join("lines.csv"), &csv).expect("write fixture");

    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/email-probe.sh");
    let out = tokio::process::Command::new("bash")
        .arg(script)
        .args(["attach", "lines.csv"])
        .current_dir(&dir.0)
        .env("MCPHOST_URL", &server.base_url)
        .env("MCPHOST_NAMESPACE", &ns)
        .env("MCPHOST_API_KEY", &key)
        .env("MCPHOST_EMAIL_INBOUND_SECRET", secret)
        .output()
        .await
        .expect("run probe");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "probe exited {:?}\nstdout: {stdout}\nstderr: {stderr}", out.status.code());
    assert!(stdout.contains("blob_id=blob_"), "stdout: {stdout}");
    assert!(stdout.contains("bytes=4096"), "stdout: {stdout}");
}
