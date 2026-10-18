//! contract: reads scripts/contract-lint.sh
//! PRD-mcphost-contract-suite-registry-axis-b AC1 -- on the Cited-tree
//! (j0yen/mcphost@9f0bd02a, before this PRD) `scripts/contract-lint.sh tests` and
//! `--fix tests` both exit 0 printing `contract-lint: ok (<n> marked files)` and no
//! `add` line, while the three registry walkers carry no `//! contract:` line and
//! no `tests/suite_contracts_*.rs` includes them: the gap is real before it is closed.
//! The tree is read from git history; a clone without that commit skips the check.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const CITED: &str = "9f0bd02a29b62c2c14263854815bb4898b81ed64";
const WALKERS: [&str; 3] = [
    "verbfwd_ac08_forward_stack_budget",
    "dryv_ac02_write_verb_registry_drift",
    "msmp_ac07_bridge_modules_list_sampling_everywhere",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn git_ok(args: &[&str]) -> bool {
    Command::new("git").args(args).current_dir(root()).output().map(|o| o.status.success()).unwrap_or(false)
}

fn cited_tree() -> Option<PathBuf> {
    if !git_ok(&["cat-file", "-e", &format!("{CITED}^{{commit}}")]) {
        eprintln!("ctrx_ac01: {CITED} not in this clone's history; skipping");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("ctrx-ac01-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let tar = dir.join("cited.tar");
    let st = Command::new("git")
        .args(["archive", "-o"])
        .arg(&tar)
        .arg(CITED)
        .current_dir(root())
        .status()
        .unwrap();
    assert!(st.success(), "git archive {CITED} failed");
    let tree = dir.join("tree");
    fs::create_dir_all(&tree).unwrap();
    let st = Command::new("tar").arg("-xf").arg(&tar).arg("-C").arg(&tree).status().unwrap();
    assert!(st.success(), "tar extract failed");
    Some(tree)
}

#[test]
fn ctrx_ac01_cited_tree_lint_is_clean_while_walkers_are_unmarked() {
    let Some(tree) = cited_tree() else { return };
    let lint = |args: &[&str]| {
        Command::new(tree.join("scripts/contract-lint.sh")).args(args).current_dir(&tree).output().unwrap()
    };
    for args in [vec!["tests"], vec!["--fix", "tests"]] {
        let out = lint(&args);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {stdout}");
        assert!(
            stdout.lines().any(|l| l.starts_with("contract-lint: ok (") && l.ends_with(" marked files)")),
            "{args:?}: {stdout}"
        );
        assert!(!stdout.contains("add"), "{args:?}: {stdout}");
    }
    let suites: Vec<String> = fs::read_dir(tree.join("tests"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("suite_contracts_"))
        .map(|n| fs::read_to_string(tree.join("tests").join(n)).unwrap())
        .collect();
    for stem in WALKERS {
        let text = fs::read_to_string(tree.join(format!("tests/{stem}.rs"))).unwrap();
        assert!(
            !text.lines().take(10).any(|l| l.starts_with("//! contract:")),
            "{stem} already carries a marker at the Cited-tree"
        );
        assert!(
            !suites.iter().any(|s| s.contains(&format!("\"{stem}.rs\""))),
            "{stem} is already in a suite_contracts_* binary at the Cited-tree"
        );
    }
    let _ = fs::remove_dir_all(tree.parent().unwrap());
}
