//! PRD-mcphost-sandbox-egress-allowlist
//! AC5 (P0) — Given any tenant and a tool with `network: "none"` or unset,
//! When it runs, Then the constructed bwrap command line contains
//! `--unshare-all` and not `--share-net`, and the unshare fallback contains
//! `-n`.
//!
//! Same "assert the constructed command line" shape as AC4's own test,
//! exercised directly at `sandbox::build_isolated_command` -- no real
//! network, no `bwrap`/user-namespace capability needed.

use mcphost::sandbox::{
    IsolationMechanism, NetworkMode, ResourceLimits, RunSpec, build_isolated_command,
};
use std::path::PathBuf;
use std::time::Duration;

fn base_spec(isolation: IsolationMechanism) -> RunSpec {
    RunSpec {
        interpreter: PathBuf::from("/usr/bin/python3"),
        interpreter_args: vec!["-I".to_string(), "-S".to_string()],
        script_path: PathBuf::from("/tmp/does-not-need-to-exist/runner.py"),
        scratch_dir: PathBuf::from("/tmp/does-not-need-to-exist"),
        read_only_dirs: vec![],
        stdin_payload: Vec::new(),
        limits: ResourceLimits {
            cpu_seconds: 5,
            memory_mb: 256,
            max_open_files: 64,
            max_file_size_mb: 16,
            max_processes: 64,
        },
        wall_clock_timeout: Duration::from_secs(7),
        network: NetworkMode::None,
        extra_env: vec![],
        isolation,
    }
}

fn argv(cmd: &tokio::process::Command) -> Vec<String> {
    cmd.as_std().get_args().map(|a| a.to_string_lossy().to_string()).collect()
}

#[test]
fn bwrap_command_unshares_all_and_never_shares_net() {
    let cmd = build_isolated_command(&base_spec(IsolationMechanism::Bwrap));
    let args = argv(&cmd);
    assert!(args.iter().any(|a| a == "--unshare-all"), "{args:?}");
    assert!(!args.iter().any(|a| a == "--share-net"), "{args:?}");
}

#[test]
fn unshare_fallback_isolates_the_network_namespace() {
    let cmd = build_isolated_command(&base_spec(IsolationMechanism::UnshareSetpriv));
    let args = argv(&cmd);
    assert!(args.iter().any(|a| a == "-n"), "{args:?}");
}
