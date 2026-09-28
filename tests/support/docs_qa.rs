//! PRD-mcphost-docs-qa-recipe: shared helpers for the `docsqa_ac*`
//! integration tests -- running `examples/docs-qa/docs-qa.sh` as a real
//! child process against an in-process `TestServer` (same "spawn the real
//! script, read its receipt" shape `mcphost_uptime_probes_ac08_*`'s Live
//! test uses for `proof.sh`, minus the `MCPHOST_LIVE` gate, since this
//! recipe's own AC1 runs the script against the test host every time, not
//! only against prod).
//!
//! `#![allow(dead_code)]`: this file is compiled fresh into whichever
//! `tests/suite_*.rs` binary each of its callers lands in (test-suite
//! consolidation, `scripts/gen-test-suites.sh`) -- dead-code analysis runs
//! per binary, so a helper only some `docsqa_ac*` files use (e.g. this
//! module's `receipt`/`run_docs_qa` in a binary that only pulled in
//! `docsqa_ac06`, which needs none of the script-running helpers) would
//! otherwise warn there even though a sibling binary uses it.
#![allow(dead_code)]

use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use tokio::process::Command;

pub fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/docs-qa")
}

pub fn corpus_dir() -> PathBuf {
    examples_dir().join("corpus")
}

#[derive(Deserialize, Clone)]
pub struct GoldQuestion {
    pub id: i64,
    #[allow(dead_code)]
    pub question: String,
    pub expected_document: String,
    #[allow(dead_code)]
    pub expected_fact: String,
}

pub fn load_gold() -> Vec<GoldQuestion> {
    let path = corpus_dir().join("gold.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

pub struct ScriptRun {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl ScriptRun {
    /// Pulls the `RECEIPT: <path>` line `docs-qa.sh` always prints on its
    /// last successful (or failed-but-past-signup) line, and parses the
    /// receipt JSON at that path.
    pub fn receipt(&self) -> Value {
        let path = self.receipt_path();
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read receipt {path}: {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse receipt {path}: {e}"))
    }

    pub fn receipt_path(&self) -> String {
        self.stdout
            .lines()
            .find_map(|l| l.strip_prefix("RECEIPT: "))
            .unwrap_or_else(|| panic!("docs-qa.sh printed no RECEIPT: line\nstdout:\n{}\nstderr:\n{}", self.stdout, self.stderr))
            .trim()
            .to_string()
    }
}

/// Runs `examples/docs-qa/docs-qa.sh <endpoint> <extra_args...>` as a real
/// child process (bash + python3, no cargo/rust in the loop) against the
/// given base MCP endpoint.
pub async fn run_docs_qa(endpoint: &str, extra_args: &[&str], envs: &[(&str, &str)]) -> ScriptRun {
    let script = examples_dir().join("docs-qa.sh");
    let mut cmd = Command::new("bash");
    cmd.arg(&script).arg(endpoint).args(extra_args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .await
        .unwrap_or_else(|e| panic!("run {}: {e}", script.display()));
    ScriptRun {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Runs a literal fenced ```bash block (e.g. pulled straight out of
/// `plugin/commands/docs-qa.md`) as a real child process, with `$1` bound to
/// `arg1` -- the same substitution a human's `/docs-qa <endpoint>` invocation
/// would perform. Unlike `run_docs_qa`, this does not know or care what the
/// block actually says: bad quoting, wrong argument order, or extra text
/// around `docs-qa.sh` all execute exactly as written and can fail the run.
pub async fn run_command_block(block: &str, arg1: &str, envs: &[(&str, &str)]) -> ScriptRun {
    let mut cmd = Command::new("bash");
    cmd.current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    cmd.arg("-c").arg(block).arg("docs-qa-command").arg(arg1);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let output = cmd
        .output()
        .await
        .unwrap_or_else(|e| panic!("run command block: {e}"));
    ScriptRun {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

pub fn scratch_receipt_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsqa-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch receipt dir");
    dir
}
