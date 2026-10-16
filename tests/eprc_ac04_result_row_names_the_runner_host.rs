//! PRD-mcphost-example-proof-runner-contract
//! AC4 — Given a run on a host named `h1` (via `MCPHOST_PROVE_RUNNER=h1`),
//! When `--report` posts, Then `admin.example_proof_report` stores
//! `runner: "h1"` and `/status.json.examples.hidden[]` carries `runner` for
//! a hidden example.

use crate::common::{ADMIN_KEY, EnvGuard};
use crate::exprf;

use mcphost::example_proofs::{RUNNER_ENV, RunOpts, run, runner_name};
use serde_json::Value;

#[tokio::test]
async fn eprc_ac04() {
    let f = exprf::start().await;
    let victim = exprf::names()[0];
    let files_dir = exprf::stub_proofs(&[victim], None);
    let opts = RunOpts {
        url: f.server.base_url.clone(),
        only: Some(victim.to_string()),
        report: true,
        dry_run: false,
        admin_key: Some(ADMIN_KEY.to_string()),
        files_dir: Some(files_dir),
    };

    let _guard = EnvGuard::set(RUNNER_ENV, "h1").await;
    assert_eq!(runner_name(), "h1");
    let (_, all_ok) = run(&opts, &mut Vec::new()).await.expect("runner");
    assert!(!all_ok);

    let row = f.server.state.db.get_example_proof(victim.to_string()).await.expect("db").expect("row");
    assert_eq!(row.runner, "h1", "{row:?}");

    let status: Value = reqwest::Client::new().get(format!("{}/status.json", f.server.base_url)).send().await.expect("GET").json().await.expect("JSON");
    let hidden = status["examples"]["hidden"].as_array().expect("hidden[]");
    assert_eq!(hidden.len(), 1, "{status}");
    assert_eq!(hidden[0]["name"], victim);
    assert_eq!(hidden[0]["runner"], "h1", "{status}");
}
