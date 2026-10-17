//! PRD-mcphost-example-recipes-conform-to-policy
//! AC3 — Given the retention proof's 300 KiB payload, When it calls
//! `host.blob.put`, Then curl receives the body from a file (`-d @` or
//! `--data-binary @-`), the host answers without a deserialize error, and
//! the proof's quota and gc checks pass.

use crate::common;
use common::TestServer;
use tokio::process::Command;

/// A `curl` shim first on PATH: logs one line per argv string
/// (`<len> <arg-prefix>`) then execs the real curl.
fn write_curl_shim(dir: &std::path::Path, log: &std::path::Path) {
    let real = String::from_utf8(std::process::Command::new("sh").args(["-c", "command -v curl"]).output().expect("which curl").stdout)
        .expect("utf8")
        .trim()
        .to_string();
    assert!(!real.is_empty(), "curl must be installed");
    let shim = dir.join("curl");
    std::fs::write(
        &shim,
        format!(
            "#!/usr/bin/env bash\nfor a in \"$@\"; do printf '%s %s\\n' \"${{#a}}\" \"${{a:0:40}}\" >> '{}'; done\nexec {} \"$@\"\n",
            log.display(),
            real
        ),
    )
    .expect("write shim");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[tokio::test]
async fn ercp_ac03() {
    let server = TestServer::start().await;
    let tmp = std::env::temp_dir().join(format!("ercp-ac03-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(&tmp).expect("mkdir");
    let log = tmp.join("curl-argv.log");
    write_curl_shim(&tmp, &log);
    let path = format!("{}:{}", tmp.display(), std::env::var("PATH").unwrap_or_default());
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/file-share/retention-proof.sh");
    let out = Command::new("bash")
        .arg(&script)
        .arg(format!("{}/mcp", server.base_url))
        .env("PATH", path)
        .output()
        .await
        .expect("run retention-proof.sh");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let argv = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tmp);

    // The body went through a file: `--data-binary` then `@-`, and no single
    // curl argv string is anywhere near the 128 KiB cap.
    assert!(argv.lines().any(|l| l.ends_with(" --data-binary")), "curl must get --data-binary:\n{argv}");
    assert!(argv.lines().any(|l| l.ends_with(" @-")), "curl must read the body from @-:\n{argv}");
    let longest = argv.lines().filter_map(|l| l.split(' ').next()?.parse::<usize>().ok()).max().unwrap_or(0);
    assert!(longest < 4096, "no curl argv string may carry the body, longest was {longest}:\n{argv}");

    let sent: usize = stdout
        .lines()
        .find_map(|l| l.strip_prefix("body_bytes="))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("proof printed no body_bytes= line:\n{stdout}"));
    assert!(sent >= 300 * 1024, "proof must have sent at least 300 KiB, sent {sent}");
    assert!(!stdout.contains("deserialize"), "host must not report a deserialize error:\n{stdout}");
    assert!(stdout.lines().any(|l| l == "CHECK blob.put succeeded: PASS"), "put must succeed:\n{stdout}");
    assert!(
        stdout.lines().any(|l| l == "quota=ok expiring=1 gc_dry_run=ok"),
        "quota and gc checks must pass:\n{stdout}"
    );
    assert!(out.status.success(), "proof must exit 0:\n{stdout}");
}
