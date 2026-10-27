//! PRD-mcphost-contract-suite-generated AC3 -- `pin-tests.sh --list` prints the
//! contract suite names read from Cargo.toml's generated `[[test]]` entries,
//! the script has no `PINS=` array, and a run executes each listed binary whole
//! with `--test-threads=1`, printing one `pin-tests: <pass|FAIL> <binary> (<n>s)`
//! line per binary and exiting 1 on any failure, 0 otherwise.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::execw;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cargo_toml_contract_suites(toml: &str) -> Vec<String> {
    let mut v: Vec<String> = toml
        .lines()
        .filter_map(|l| l.strip_prefix("name = \"suite_contracts_"))
        .map(|r| format!("suite_contracts_{}", r.trim_end_matches('"')))
        .collect();
    v.sort();
    v
}

#[test]
fn ctrs_ac03_list_is_read_from_cargo_toml_and_script_has_no_pins_array() {
    let out = Command::new(root().join("scripts/pin-tests.sh"))
        .arg("--list")
        .current_dir(root())
        .output()
        .unwrap();
    assert!(out.status.success());
    let listed: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect();
    let expect = cargo_toml_contract_suites(&fs::read_to_string(root().join("Cargo.toml")).unwrap());
    assert!(!expect.is_empty());
    assert_eq!(listed, expect);
    let script = fs::read_to_string(root().join("scripts/pin-tests.sh")).unwrap();
    assert!(!script.contains("PINS="), "pin-tests.sh must not carry a hand PINS= array");
}

fn run_with_fake_cargo(fail_on: Option<&str>) -> (std::process::Output, String) {
    let dir = std::env::temp_dir().join(format!("ctrs-ac03-{}-{}", std::process::id(), fail_on.unwrap_or("ok")));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("scripts")).unwrap();
    fs::copy(root().join("scripts/pin-tests.sh"), dir.join("scripts/pin-tests.sh")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[[test]]\nname = \"suite_contracts_core_01\"\n\n[[test]]\nname = \"suite_core_01\"\n\n[[test]]\nname = \"suite_contracts_sandbox_01\"\n",
    )
    .unwrap();
    let log = dir.join("calls.log");
    let fake = dir.join("fakecargo.sh");
    fs::write(
        &fake,
        format!(
            "#!/usr/bin/env bash\necho \"$*\" >> {}\ncase \"$*\" in *\"${{FAIL_ON:-@@none@@}}\"*) echo boom; exit 101;; esac\nexit 0\n",
            log.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for p in [dir.join("scripts/pin-tests.sh"), fake.clone()] {
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let mut cmd = execw::script(&dir.join("scripts/pin-tests.sh"));
    cmd.current_dir(&dir).env("CARGO", &fake);
    if let Some(f) = fail_on {
        cmd.env("FAIL_ON", f);
    }
    let out = cmd.output().unwrap();
    let calls = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_dir_all(&dir);
    (out, calls)
}

#[test]
fn ctrs_ac03_runs_each_binary_whole_single_threaded_and_exits_zero() {
    let (out, calls) = run_with_fake_cargo(None);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // sandbox (server-spawning) stays single-threaded; core (script drift checks) is parallel
    for (bin, threads) in [("suite_contracts_core_01", 8), ("suite_contracts_sandbox_01", 1)] {
        assert!(stdout.contains(&format!("pin-tests: pass {bin} (")), "{stdout}");
        assert!(
            calls.lines().any(|l| l.contains(&format!("--test {bin} -- --test-threads={threads}"))),
            "binary must run whole (no filter) with --test-threads={threads}:\n{calls}"
        );
    }
    assert!(!stdout.contains("suite_core_01"), "non-contract suites must not run");
    assert_eq!(calls.lines().count(), 2);
}

#[test]
fn ctrs_ac03_any_failure_exits_one_and_prints_fail_line() {
    let (out, _calls) = run_with_fake_cargo(Some("suite_contracts_sandbox_01"));
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("pin-tests: FAIL suite_contracts_sandbox_01 ("), "{stdout}");
    assert!(stdout.contains("pin-tests: pass suite_contracts_core_01 ("), "{stdout}");
}
