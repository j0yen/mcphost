//! AC2 — Given the production registry from `production_registry()`, When
//! `run_contract` runs over every name, Then `echo`, `http`, `python`,
//! `chain`, `wasm` each pass all five cases or skip one with a reason the
//! contract states.

use mcphost::kinds::conformance::{CaseOutcome, run_contract_report};
use mcphost::kinds::production_registry;
use mcphost::sandbox;

/// `python`'s call, error, budget and secret cases run a real sandboxed
/// tool, which needs unprivileged user namespaces and `uv` (same gate as
/// `ac17_kind_conformance`'s python test).
fn sandbox_unavailable() -> bool {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return true;
    }
    let uv_present = std::process::Command::new("uv")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !uv_present {
        println!("skipped python: no uv");
    }
    !uv_present
}

#[tokio::test]
async fn kcon_ac02_every_registered_kind_passes() {
    let registry = production_registry();
    let names = registry.names();
    assert_eq!(names, ["chain", "echo", "http", "python", "wasm"]);

    let skip_python = sandbox_unavailable();
    for name in names {
        if name == "python" && skip_python {
            continue;
        }
        let kind = registry.get(name).expect("registered name resolves");
        let report = run_contract_report(kind.as_ref()).await;
        assert_eq!(report.len(), 5, "kind={name}: five cases report");
        for r in &report {
            match &r.outcome {
                CaseOutcome::Pass => {}
                CaseOutcome::Skip(reason) => {
                    assert!(!reason.trim().is_empty(), "kind={name} case={} skips without a reason", r.case)
                }
                CaseOutcome::Fail(reason) => panic!("{reason}"),
            }
        }
    }
}
