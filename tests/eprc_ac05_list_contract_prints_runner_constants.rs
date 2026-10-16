//! PRD-mcphost-example-proof-runner-contract
//! AC5 — Given `examples prove --list-contract`, When run, Then it prints the
//! argv shape, the env names and the dry variable, and a test asserts the
//! printed names equal the constants the runner uses.

use mcphost::example_proofs::{ARGV_SHAPE, DRY_ENV, RUNNER_ENV, URL_ENV, RunOpts, run};

#[tokio::test]
async fn eprc_ac05() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost")).args(["examples", "prove", "--list-contract"]).output().expect("spawn mcphost");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();

    let argv = text.lines().find_map(|l| l.strip_prefix("argv: ")).expect("argv line");
    assert_eq!(argv, ARGV_SHAPE);
    let env_names: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("env ")).filter_map(|l| l.split(':').next()).collect();
    assert_eq!(env_names, [URL_ENV, DRY_ENV, RUNNER_ENV]);
    assert_eq!(text, mcphost::example_proofs::contract_text());

    // The runner really exports the URL under the printed name, with the URL as $1.
    let files = std::env::temp_dir().join(format!("eprc-ac05-{}-{}", std::process::id(), mcphost::state::now_unix_ms()));
    std::fs::create_dir_all(files.join("retrieval")).expect("dir");
    let seen = files.join("seen.txt");
    std::fs::write(files.join("retrieval/proof.sh"), format!("echo \"$1 ${URL_ENV}\" > '{}'\n", seen.display())).expect("stub");
    let opts = RunOpts { url: "http://h/mcp".to_string(), only: Some("retrieval".to_string()), report: false, dry_run: false, admin_key: None, files_dir: Some(files) };
    run(&opts, &mut Vec::new()).await.expect("runner");
    assert_eq!(std::fs::read_to_string(seen).expect("seen").trim(), "http://h/mcp http://h/mcp");
}
