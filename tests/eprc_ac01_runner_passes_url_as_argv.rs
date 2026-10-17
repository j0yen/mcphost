//! PRD-mcphost-example-proof-runner-contract
//! AC1 — Given the embedded index, When `examples prove --live <url> --only
//! retrieval` runs, Then the proof receives `<url>` as `$1` and
//! `MCPHOST_URL`, and `retrieval` no longer prints usage (it runs
//! `--create-check <url>` by default).

use mcphost::example_proofs::{RunOpts, run};

const URL: &str = "http://127.0.0.1:1/mcp";

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("eprc-ac01-{tag}-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(&d).expect("tmp dir");
    d
}

#[tokio::test]
async fn eprc_ac01() {
    // The runner: a stub retrieval proof records its `$1` and `MCPHOST_URL`.
    let files = tmp("files");
    let seen = files.join("seen.txt");
    std::fs::create_dir_all(files.join("retrieval")).expect("stub dir");
    std::fs::write(files.join("retrieval/proof.sh"), format!("echo \"$1|$MCPHOST_URL\" > '{}'\n", seen.display())).expect("stub");
    let opts = RunOpts { url: URL.to_string(), only: Some("retrieval".to_string()), report: false, dry_run: false, admin_key: None, files_dir: Some(files) };
    let (runs, all_ok) = run(&opts, &mut Vec::new()).await.expect("runner");
    assert!(all_ok && runs.len() == 1, "{runs:?}");
    assert_eq!(std::fs::read_to_string(&seen).expect("stub ran").trim(), format!("{URL}|{URL}"));

    // The real embedded retrieval proof, spawned the runner's way, no longer
    // prints usage: it reaches `--create-check` (and stops at the missing key file).
    let ex = mcphost::examples::EXAMPLES.iter().find(|e| e.name == "retrieval").expect("retrieval");
    let dir = tmp("real");
    for (rel, bytes) in ex.files {
        std::fs::write(dir.join(rel), bytes).expect("extract");
    }
    let out = std::process::Command::new("bash")
        .arg(ex.proof)
        .arg(URL)
        .current_dir(&dir)
        .env("MCPHOST_URL", URL)
        .env("HOME", &dir)
        .env_remove("MCPHOST_JOE_TEST_ENV")
        .output()
        .expect("spawn");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!log.contains("usage"), "retrieval printed usage: {log}");
    assert!(log.contains("cannot read"), "expected create-check to run: {log}");
}
