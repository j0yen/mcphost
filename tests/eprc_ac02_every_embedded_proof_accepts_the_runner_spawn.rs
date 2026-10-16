//! PRD-mcphost-example-proof-runner-contract
//! AC2 — Given every `Example` in the embedded index, When the contract test
//! spawns `bash <proof> <url>` with `MCPHOST_URL=<url>` and
//! `MCPHOST_PROVE_DRY=1`, Then each exits 0 and prints a line starting
//! `plan:`; and Given a fixture proof that prints usage and exits 2, Then the
//! test fails naming that example.

use mcphost::examples::{EXAMPLES, Example};

const URL: &str = "http://127.0.0.1:1/mcp";

/// Spawn `bash <proof> <url>` exactly as the runner does, in dry mode, from
/// a directory holding `files`. `Err` names the example and says why.
fn spawn_dry(name: &str, proof: &str, files: &[(&str, &[u8])]) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("eprc-ac02-{name}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    for (rel, bytes) in files {
        let target = dir.join(rel);
        std::fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        std::fs::write(&target, bytes).expect("write");
    }
    let out = std::process::Command::new("bash")
        .arg(proof)
        .arg(URL)
        .current_dir(&dir)
        .env("MCPHOST_URL", URL)
        .env("MCPHOST_PROVE_DRY", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn bash");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(format!("example `{name}`: exit {:?}\n{stdout}{stderr}", out.status.code()));
    }
    if !stdout.lines().any(|l| l.starts_with("plan:")) {
        return Err(format!("example `{name}`: no `plan:` line in stdout\n{stdout}{stderr}"));
    }
    Ok(())
}

fn check(e: &Example) -> Result<(), String> {
    spawn_dry(e.name, e.proof, e.files)
}

#[test]
fn eprc_ac02() {
    for e in EXAMPLES {
        check(e).unwrap_or_else(|msg| panic!("{msg}"));
    }

    // Control: a fixture proof that prints usage and exits 2 is rejected by name.
    let usage: &[(&str, &[u8])] = &[("proof.sh", b"echo 'usage: proof.sh --mode' >&2\nexit 2\n")];
    let err = spawn_dry("usage-fixture", "proof.sh", usage).expect_err("usage proof must fail the contract");
    assert!(err.contains("example `usage-fixture`") && err.contains("exit Some(2)") && err.contains("usage:"), "{err}");
}
