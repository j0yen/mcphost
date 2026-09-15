//! PRD-mcphost-checkcompat-port-race AC3/AC4: `/healthz` carries
//! `X-Mcphost-Compat-Token` matching `MCPHOST_COMPAT_TOKEN` when it is set
//! in the serving process's own env (AC3, requirement 2/5), and carries no
//! such header at all when it is unset (AC4) -- production's env never
//! sets it, so production responses are byte-for-byte unchanged. Each half
//! spawns a real `mcphost serve` subprocess (separate processes, so
//! there's no shared-env race between the two cases) rather than mutating
//! `std::env` in-process, since this test binary may run other tests in
//! parallel threads.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn mcphost_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcphost"))
}

fn scratch_data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-checkcompat-race-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("create scratch data dir");
    dir
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn wait_for_ok(client: &reqwest::Client, base_url: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(resp) = client.get(format!("{base_url}/healthz")).send().await
            && resp.status().is_success()
        {
            return;
        }
        assert!(Instant::now() < deadline, "server at {base_url} did not become ready in time");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn spawn_serve(data_dir: &std::path::Path, port: u16, token: Option<&str>) -> ServerGuard {
    let mut cmd = Command::new(mcphost_bin());
    cmd.arg("serve")
        .env("MCPHOST_DATA_DIR", data_dir)
        .env("MCPHOST_BIND", format!("127.0.0.1:{port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match token {
        Some(t) => {
            cmd.env("MCPHOST_COMPAT_TOKEN", t);
        }
        None => {
            cmd.env_remove("MCPHOST_COMPAT_TOKEN");
        }
    }
    ServerGuard(cmd.spawn().expect("spawn mcphost serve"))
}

#[tokio::test]
async fn healthz_carries_the_token_header_only_when_compat_token_is_set() {
    let client = reqwest::Client::new();

    // AC3: token set -> header present and matching.
    let port = free_port();
    let data_dir = scratch_data_dir("ac03tok");
    let with_token = spawn_serve(&data_dir, port, Some("test-token-abc123"));
    let base = format!("http://127.0.0.1:{port}");
    wait_for_ok(&client, &base).await;
    let resp = client
        .get(format!("{base}/healthz"))
        .send()
        .await
        .expect("GET /healthz (with token)");
    let header = resp
        .headers()
        .get("X-Mcphost-Compat-Token")
        .map(|v| v.to_str().unwrap().to_string());
    assert_eq!(
        header.as_deref(),
        Some("test-token-abc123"),
        "AC3: /healthz must carry the matching token header"
    );
    drop(with_token);
    let _ = std::fs::remove_dir_all(&data_dir);

    // AC4: token unset -> header absent entirely.
    let port2 = free_port();
    let data_dir2 = scratch_data_dir("ac04notok");
    let without_token = spawn_serve(&data_dir2, port2, None);
    let base2 = format!("http://127.0.0.1:{port2}");
    wait_for_ok(&client, &base2).await;
    let resp2 = client
        .get(format!("{base2}/healthz"))
        .send()
        .await
        .expect("GET /healthz (without token)");
    assert!(
        resp2.headers().get("X-Mcphost-Compat-Token").is_none(),
        "AC4: token header must be absent when MCPHOST_COMPAT_TOKEN is unset"
    );
    drop(without_token);
    let _ = std::fs::remove_dir_all(&data_dir2);
}
