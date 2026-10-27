//! PRD-mcphost-contract-suite-markers AC2 -- the four named tests carry
//! `//! contract:` in their first 10 lines, `scripts/gen-test-suites.sh --check`
//! passes, and each file is included by exactly one `suite_contracts_*` binary
//! that `scripts/pin-tests.sh --list` names.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const FOUR: [&str; 4] = [
    "suite_ac4_archived_prd_pointers_resolve",
    "argmsg_ac06_sites_render_from_one_args_error",
    "sfc_ac04_stray_notify_panics_in_debug",
    "checkcompat_race_ac03_inherited_listener_token_matches",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run(script: &str, arg: &str) -> (i32, String) {
    let out = Command::new(root().join("scripts").join(script))
        .arg(arg)
        .current_dir(root())
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn ctrm_ac02_four_files_carry_the_marker_in_the_first_ten_lines() {
    for name in FOUR {
        let src = fs::read_to_string(root().join("tests").join(format!("{name}.rs"))).unwrap();
        assert!(
            src.lines().take(10).any(|l| l.starts_with("//! contract:")),
            "{name} lacks `//! contract:` in its first 10 lines"
        );
    }
}

#[test]
fn ctrm_ac02_gen_check_passes_and_each_file_is_in_exactly_one_listed_contract_suite() {
    let (rc, out) = run("gen-test-suites.sh", "--check");
    assert_eq!(rc, 0, "{out}");
    let (rc, list) = run("pin-tests.sh", "--list");
    assert_eq!(rc, 0, "{list}");
    let listed: Vec<&str> = list.lines().map(str::trim).collect();
    for name in FOUR {
        let needle = format!("mod {name};");
        let mut homes = Vec::new();
        for entry in fs::read_dir(root().join("tests")).unwrap() {
            let p = entry.unwrap().path();
            let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
            if !stem.starts_with("suite_") || p.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            if fs::read_to_string(&p).unwrap().contains(&needle) {
                homes.push(stem);
            }
        }
        assert_eq!(homes.len(), 1, "{name} is included by {homes:?}");
        assert!(homes[0].starts_with("suite_contracts_"), "{name} lives in {}", homes[0]);
        assert!(listed.contains(&homes[0].as_str()), "{} not in pin-tests --list: {list}", homes[0]);
    }
}
