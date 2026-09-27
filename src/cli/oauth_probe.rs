//! `mcphost oauth-probe`: runs the same scenario table `tests/oauthconf_*`
//! runs in-process, against a live `--url` (requirement 3) -- goal 3, "the
//! same scenarios runnable against prod with a receipt".

use std::path::{Path, PathBuf};

use crate::oauthclient::{self, ScenarioResult, Verdict};

pub struct Args {
    pub url: String,
    pub scenarios: Vec<String>,
    pub json: bool,
    pub receipt: Option<PathBuf>,
}

/// Runs every scenario (or, with `scenarios` non-empty, only the named
/// ones) against `args.url`, prints the table (`--json` or plain text),
/// writes a `--receipt` markdown file when asked, and returns the process
/// exit code: 0 when every scenario reads `pass`/`unsupported`, 1 on any
/// `fail`. AC6: neither the printed table nor the receipt is built from
/// anything but [`oauthclient::ScenarioResult`], which never carries a
/// code/token value in the first place -- there is nothing here that could
/// leak one.
pub async fn run(http: &reqwest::Client, args: &Args) -> anyhow::Result<i32> {
    let mut results = oauthclient::run_all(http, &args.url).await;
    if !args.scenarios.is_empty() {
        results.retain(|r| args.scenarios.iter().any(|s| s == &r.name));
    }
    apply_test_only_force_fail(&mut results);

    let exit_code = if results.iter().all(|r| matches!(r.verdict, Verdict::Pass | Verdict::Unsupported)) { 0 } else { 1 };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        for r in &results {
            println!("{:<48} {:<16} {:?}", r.name, r.family, r.verdict);
        }
    }
    if let Some(path) = &args.receipt {
        write_receipt(path, &results)?;
    }
    Ok(exit_code)
}

/// AC5's own test-only hook: `$MCPHOST_OAUTHCONF_FORCE_FAIL=<scenario
/// name>` flips one scenario's verdict to `fail` after the real run, so a
/// test can prove the CLI's exit-code behavior without the host actually
/// misbehaving. Inert unless set; [`oauthclient::run_all`] itself never
/// consults it, so no production probe run is affected by an unset var.
fn apply_test_only_force_fail(results: &mut [ScenarioResult]) {
    let Ok(name) = std::env::var("MCPHOST_OAUTHCONF_FORCE_FAIL") else { return };
    for r in results.iter_mut() {
        if r.name == name {
            r.verdict = Verdict::Fail;
        }
    }
}

/// Pure rendering half of `write_receipt`, split out so AC6's test can
/// prove the receipt never carries a secret without also touching the
/// filesystem. Only `name`/`family`/`verdict` -- `records` (the only place
/// a secret could reach, in principle, via `headers_of_interest`/`reason`)
/// never feeds the receipt at all.
pub fn render_receipt(results: &[ScenarioResult]) -> String {
    let mut out = String::from("# oauth-probe receipt\n\n| scenario | family | verdict |\n|---|---|---|\n");
    for r in results {
        out.push_str(&format!("| {} | {} | {:?} |\n", r.name, r.family, r.verdict));
    }
    out
}

fn write_receipt(path: &Path, results: &[ScenarioResult]) -> anyhow::Result<()> {
    let out = render_receipt(results);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, out)?;
    Ok(())
}
