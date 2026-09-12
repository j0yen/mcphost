//! PRD-mcphost-sandbox-ready
//! AC9 (P1) -- Given each of the five failure classes (uid_map EPERM,
//! loopback EPERM, missing bwrap, missing `/usr/bin/python3`, wall-clock
//! timeout) injected in turn, When the self-test runs, Then `sandbox_detail`
//! starts with the matching token `userns_denied`, `userns_denied`,
//! `binary_missing`, `interpreter_missing`, `timeout`.

use crate::common;
use common::fake_interpreter_failing;
use mcphost::kinds::python::PythonKind;
use mcphost::sandbox;

#[tokio::test]
async fn each_canonical_failure_message_classifies_to_its_documented_token() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }

    // uid_map EPERM (technical considerations: `unshare -Urn -- /bin/true`).
    {
        let data_dir = common::TempDataDir::new();
        let py = PythonKind::for_test_with_selftest(&data_dir.0, 300);
        py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
            "write failed /proc/self/uid_map: Operation not permitted",
        )));
        let status = py.run_startup_selftest().await;
        assert!(
            status.detail.starts_with("userns_denied:"),
            "uid_map case: {}",
            status.detail
        );
    }

    // loopback EPERM (technical considerations: `bwrap --unshare-all ...`).
    {
        let data_dir = common::TempDataDir::new();
        let py = PythonKind::for_test_with_selftest(&data_dir.0, 300);
        py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
            "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted",
        )));
        let status = py.run_startup_selftest().await;
        assert!(
            status.detail.starts_with("userns_denied:"),
            "loopback case: {}",
            status.detail
        );
    }

    // Missing bwrap/unshare (the RunSpec's own top-level command can't be
    // spawned at all -- see `SelftestInterpreter::MissingBinarySpawnFailure`'s
    // doc comment for why this crate can't genuinely uninstall bwrap here).
    {
        let data_dir = common::TempDataDir::new();
        let py = PythonKind::for_test_with_selftest(&data_dir.0, 300);
        py.set_selftest_missing_binary_for_test();
        let status = py.run_startup_selftest().await;
        assert!(
            status.detail.starts_with("binary_missing:"),
            "missing wrapper case: {}",
            status.detail
        );
    }

    // Missing /usr/bin/python3 (bwrap runs, then fails to execvp it).
    {
        let data_dir = common::TempDataDir::new();
        let py = PythonKind::for_test_with_selftest(&data_dir.0, 300);
        py.set_selftest_interpreter_for_test(Some(&fake_interpreter_failing(
            "bwrap: execvp /usr/bin/python3: No such file or directory",
        )));
        let status = py.run_startup_selftest().await;
        assert!(
            status.detail.starts_with("interpreter_missing:"),
            "missing interpreter case: {}",
            status.detail
        );
    }

    // Wall-clock timeout.
    {
        let data_dir = common::TempDataDir::new();
        let py = PythonKind::for_test_with_selftest(&data_dir.0, 300);
        py.set_selftest_interpreter_for_test(Some("#!/bin/sh\ncat >/dev/null\nsleep 30\n"));
        let status = py.run_startup_selftest().await;
        assert!(
            status.detail.starts_with("timeout:"),
            "timeout case: {}",
            status.detail
        );
    }
}
