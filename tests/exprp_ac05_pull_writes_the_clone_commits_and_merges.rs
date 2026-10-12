//! PRD-mcphost-example-from-published-tool
//! AC5 — Given one approved proposal and a clone on a fixture host, When
//! `mcphost examples proposals pull --repo <clone>` runs with `gh` stubbed,
//! Then the clone has `examples/<name>/{example.toml,tools/,proof.sh}`,
//! `mcphost examples check` passes on it, one commit exists, and the
//! proposal is `merged` with the PR URL.

use crate::exprp;

use crate::common::{ADMIN_KEY, McpClient, extract_structured};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use tokio::process::Command;

const PR_URL: &str = "https://github.com/j0yen/mcphost/pull/777";

fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    let out = StdCommand::new(program).current_dir(dir).args(args).output().unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(out.status.success(), "{program} {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A clone with one commit on `main`, a local bare `origin`, and a `gh` stub.
fn fixture_clone(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let origin = root.join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    run(&origin, "git", &["init", "-q", "--bare", "-b", "main"]);
    let clone = root.join("clone");
    std::fs::create_dir_all(clone.join("examples")).unwrap();
    std::fs::write(clone.join("examples/.gitkeep"), "").unwrap();
    run(&clone, "git", &["init", "-q", "-b", "main"]);
    run(&clone, "git", &["config", "user.email", "t@example.com"]);
    run(&clone, "git", &["config", "user.name", "t"]);
    run(&clone, "git", &["remote", "add", "origin", origin.to_str().unwrap()]);
    run(&clone, "git", &["add", "-A"]);
    run(&clone, "git", &["commit", "-q", "-m", "initial"]);
    run(&clone, "git", &["push", "-q", "-u", "origin", "main"]);
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(&gh, format!("#!/bin/sh\necho \"$@\" >> \"{}/gh.log\"\necho {PR_URL}\n", root.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    (clone, bin, root.join("gh.log"))
}

#[tokio::test]
async fn pull_writes_commits_opens_the_pr_and_marks_the_proposal_merged() {
    let f = exprp::start().await;
    exprp::publish_http(&f.client, "probe", exprp::probe_spec(&f.upstream)).await;
    let proposal = exprp::propose(&f.client, exprp::propose_args("probe")).await.unwrap_or_else(|e| panic!("propose: {} {}", e.code, e.message));
    let id = proposal["proposal_id"].as_i64().unwrap();
    let admin = McpClient::with_bearer(&f.server.base_url, ADMIN_KEY);
    admin.tools_call("admin.example_proposals", json!({"action": "approve", "id": id})).await.expect("approve");

    let root = std::env::temp_dir().join(format!("exprp-ac05-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (clone, bin, gh_log) = fixture_clone(&root);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["examples", "proposals", "pull", "--repo"])
        .arg(&clone)
        .args(["--url", &f.server.base_url])
        .env("MCPHOST_ADMIN_KEY", ADMIN_KEY)
        .env("PATH", &path)
        .output()
        .await
        .expect("run pull");
    assert!(out.status.success(), "pull failed: {} {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));

    let example = clone.join("examples/probe");
    for rel in ["example.toml", "tools/probe.json", "proof.sh"] {
        assert!(example.join(rel).is_file(), "{rel} missing from the clone");
    }
    let manifest = std::fs::read_to_string(example.join("example.toml")).unwrap();
    assert!(manifest.contains(&format!("proposal_id = {id}")), "provenance names the proposal: {manifest}");

    let check = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["examples", "check", "--dir"])
        .arg(clone.join("examples"))
        .output()
        .await
        .expect("run check");
    assert_eq!(check.status.code(), Some(0), "check stderr: {}", String::from_utf8_lossy(&check.stderr));

    assert_eq!(run(&clone, "git", &["rev-list", "--count", "main..HEAD"]), "1", "exactly one commit on the PR branch");
    let subject = run(&clone, "git", &["log", "-1", "--format=%s"]);
    assert_eq!(subject, format!("examples: add probe (proposal {id} from {})", f.ns));
    let index = std::fs::read_to_string(clone.join("examples/index.json")).unwrap();
    assert!(index.contains("\"probe\"") && index.contains(&f.ns), "index.json carries the provenance tenant");

    let log = std::fs::read_to_string(gh_log).expect("gh was called");
    assert!(log.contains("pr create") && log.contains(&format!("examples/probe-proposal-{id}")), "gh args: {log}");

    let row = f.server.state.db.get_example_proposal(id).await.unwrap().expect("row");
    assert_eq!(row.status, "merged");
    assert_eq!(row.pr_url.as_deref(), Some(PR_URL));
    let status = extract_structured(&f.client.tools_call("host.example.proposal_status", json!({"id": id})).await.expect("status"));
    assert_eq!(status["status"], "merged");
    assert_eq!(status["pr_url"], PR_URL);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_gh_failure_leaves_the_proposal_approved() {
    let f = exprp::start().await;
    exprp::publish_http(&f.client, "probe", exprp::probe_spec(&f.upstream)).await;
    let id = exprp::propose(&f.client, exprp::propose_args("probe")).await.unwrap()["proposal_id"].as_i64().unwrap();
    let admin = McpClient::with_bearer(&f.server.base_url, ADMIN_KEY);
    admin.tools_call("admin.example_proposals", json!({"action": "approve", "id": id})).await.expect("approve");

    let root = std::env::temp_dir().join(format!("exprp-ac05-fail-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (clone, bin, _log) = fixture_clone(&root);
    std::fs::write(bin.join("gh"), "#!/bin/sh\necho boom >&2\nexit 1\n").unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["examples", "proposals", "pull", "--repo"])
        .arg(&clone)
        .args(["--url", &f.server.base_url])
        .env("MCPHOST_ADMIN_KEY", ADMIN_KEY)
        .env("PATH", &path)
        .output()
        .await
        .expect("run pull");
    assert_eq!(out.status.code(), Some(1), "a failed PR exits 1");
    let row = f.server.state.db.get_example_proposal(id).await.unwrap().expect("row");
    assert_eq!(row.status, "approved", "no PR URL recorded, so not merged");
    assert!(row.pr_url.is_none());
    let _ = std::fs::remove_dir_all(&root);
}
