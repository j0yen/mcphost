//! PRD-mcphost-email-send-live-proof
//! AC1 (P0) -- Given `scripts/email-probe.sh` run with no verb or an unknown
//! verb, When it runs, Then it prints a usage line naming `attach` and `send`
//! and exits 1; Given `send` with `MCPHOST_URL` pointing at the in-process
//! server and `MCPHOST_API_KEY` unset, When it runs, Then it creates its own
//! tenant via `signup {handoff: true}` and `host.redeem`, never reads
//! `MCPHOST_API_KEY`, and `host.self.offboard` is called on that tenant on
//! every exit path including a step that fails mid-run.

use crate::common;
use common::TestServer;

/// `(stdout, stderr, exit code)` of one script run; `env` is added to a
/// cleared-down environment (only PATH/HOME survive).
async fn run_probe(args: Vec<String>, env: Vec<(String, String)>) -> (String, String, i32) {
    tokio::task::spawn_blocking(move || {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/email-probe.sh");
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(script).args(args).env_clear();
        for key in ["PATH", "HOME"] {
            if let Ok(v) = std::env::var(key) {
                cmd.env(key, v);
            }
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run script");
        (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.status.code().unwrap_or(-1),
        )
    })
    .await
    .expect("join")
}

fn line<'a>(stdout: &'a str, prefix: &str) -> &'a str {
    stdout.lines().find(|l| l.starts_with(prefix)).unwrap_or_else(|| panic!("no `{prefix}` line in:\n{stdout}"))
}

#[tokio::test]
async fn emslp_ac01_no_verb_or_unknown_verb_prints_usage_and_exits_1() {
    for args in [vec![], vec!["bogus".to_string()]] {
        let (_out, err, rc) = run_probe(args, vec![]).await;
        assert_eq!(rc, 1, "{err}");
        assert!(err.contains("usage:") && err.contains("attach") && err.contains("send"), "{err}");
    }
}

#[tokio::test]
async fn emslp_ac01_send_owns_its_tenant_and_offboards_it_when_a_step_fails() {
    let server = TestServer::start().await;
    // MCPHOST_API_KEY is poison: a script that read it would authenticate as nobody.
    let env = vec![
        ("MCPHOST_URL".to_string(), server.base_url.clone()),
        ("MCPHOST_API_KEY".to_string(), "poison-key-must-not-be-read".to_string()),
    ];

    // Failure path 1: no verification path is available (exit 2, after signup).
    let (out, err, rc) =
        run_probe(vec!["send".into(), "--verify".into(), "admin".into()], env.clone()).await;
    assert_eq!(rc, 2, "{out}{err}");
    assert!(out.contains("status=error reason=verify_path_unavailable"), "{out}");
    assert_eq!(line(&out, "offboarded="), "offboarded=true", "{out}");
    let ns = line(&out, "tenant=").trim_start_matches("tenant=").to_string();
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("db").expect("tenant row");
    assert!(tenant.disabled, "{ns} must be offboarded");
    assert_eq!(tenant.disabled_reason.as_deref(), Some("self_offboard"));

    // Failure path 2: a step fails mid-run (wrong admin key -> verify step refused).
    let mut env2 = env.clone();
    env2.push(("MCPHOST_ADMIN_KEY".to_string(), "not-the-admin-key".to_string()));
    let (out, err, rc) = run_probe(vec!["send".into(), "--verify".into(), "admin".into()], env2).await;
    assert_eq!(rc, 1, "{out}{err}");
    assert!(out.contains("status=error reason=admin_verify"), "{out}");
    assert_eq!(line(&out, "offboarded="), "offboarded=true", "{out}");
    let ns2 = line(&out, "tenant=").trim_start_matches("tenant=").to_string();
    assert_ne!(ns, ns2, "each run signs up its own tenant");
    let tenant = server.state.db.find_tenant_by_namespace(ns2).await.expect("db").expect("tenant row");
    assert!(tenant.disabled);

    // Success path: --verify none runs to a verdict, still offboards.
    let (out, err, rc) = run_probe(vec!["send".into(), "--verify".into(), "none".into()], env).await;
    assert_eq!(rc, 1, "{out}{err}");
    assert_eq!(line(&out, "verify="), "verify=none");
    assert_eq!(line(&out, "offboarded="), "offboarded=true", "{out}");
}
