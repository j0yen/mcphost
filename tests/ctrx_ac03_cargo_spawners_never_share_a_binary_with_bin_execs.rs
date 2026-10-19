//! contract: reads scripts/ci-suite-weights.json
//! PRD-mcphost-contract-suite-registry-axis-b AC3 -- `scripts/gen-test-suites.sh`
//! derives `spawns_cargo` (`Command::new(env!("CARGO"))` / `Command::new("cargo")`)
//! and `execs_bin` (`CARGO_BIN_EXE_`) per test file. On the committed tree
//! `ctrs_ac02` and the `exmf_*` files live in different generated binaries and no
//! generated binary holds both kinds; `--check` on a fixture suite mixing one of
//! each exits 1 naming both files and the suite (CI probe 39163232421: `exmf`
//! `NotFound` inside `ctrs_ac02`'s cargo window).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// Built from pieces so this file's own source is neither kind of file.
fn spawn_pat() -> String {
    format!("Command::new(env!(\"{}\"))", "CARGO")
}

fn exec_pat() -> String {
    format!("{}_{}", "CARGO_BIN", "EXE_mcphost")
}

/// The file with `//` comment lines removed, as the generator reads it.
fn code_only(text: &str) -> String {
    text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

/// (suite name, member stems) for every generated tests/suite_*.rs.
fn suites(dir: &Path) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir.join("tests")).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("suite_") && name.ends_with(".rs")) {
            continue;
        }
        let text = fs::read_to_string(e.path()).unwrap();
        let members = text
            .lines()
            .filter_map(|l| l.strip_prefix("#[path = \"")?.strip_suffix("\"]"))
            .filter(|m| !m.contains('/'))
            .map(|m| m.trim_end_matches(".rs").to_string())
            .collect();
        out.push((name.trim_end_matches(".rs").to_string(), members));
    }
    out
}

fn holder(all: &[(String, Vec<String>)], stem: &str) -> String {
    all.iter().find(|(_, m)| m.iter().any(|s| s == stem)).map(|(n, _)| n.clone()).unwrap_or_else(|| panic!("{stem} is in no suite"))
}

#[test]
fn ctrx_ac03_ctrs_ac02_and_the_exmf_files_are_in_different_binaries() {
    let all = suites(&root());
    let spawner = holder(&all, "ctrs_ac02_six_files_carry_marker_and_keep_tests");
    let exmf: Vec<String> = fs::read_dir(root().join("tests"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("exmf_") && n.ends_with(".rs"))
        .collect();
    assert!(!exmf.is_empty());
    for f in exmf {
        let h = holder(&all, f.trim_end_matches(".rs"));
        assert_ne!(h, spawner, "{f} shares {spawner} with ctrs_ac02");
    }
}

#[test]
fn ctrx_ac03_no_generated_binary_holds_both_kinds() {
    let (spawn, exec) = (spawn_pat(), exec_pat());
    let mut spawners = 0;
    for (suite, members) in suites(&root()) {
        let kinds: Vec<(bool, bool)> = members
            .iter()
            .map(|m| {
                let t = code_only(&fs::read_to_string(root().join("tests").join(format!("{m}.rs"))).unwrap());
                (t.contains(&spawn) || t.contains("Command::new(\"cargo\")"), t.contains(&exec))
            })
            .collect();
        spawners += kinds.iter().filter(|k| k.0).count();
        let (s, e) = (kinds.iter().any(|k| k.0 && !k.1), kinds.iter().any(|k| k.1 && !k.0));
        assert!(!(s && e), "{suite} holds a cargo spawner and a CARGO_BIN_EXE_ file");
    }
    assert!(spawners >= 2, "the committed tree has spawners (ctrs_ac02, ctrs_ac06)");
}

fn fixture() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ctrx-ac03-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("scripts")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    for s in ["gen-test-suites.sh", "ci-test-partition.sh"] {
        fs::copy(root().join("scripts").join(s), dir.join("scripts").join(s)).unwrap();
    }
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"fx\"\nversion = \"0.0.0\"\n").unwrap();
    fs::write(dir.join("scripts/ci-suite-weights.json"), "{}\n").unwrap();
    fs::write(dir.join("tests/zz_spawn.rs"), format!("fn f() {{ let _ = {}; }}\n#[test]\nfn t() {{}}\n", spawn_pat())).unwrap();
    fs::write(dir.join("tests/zz_exec.rs"), format!("fn f() {{ let _ = env!(\"{}\"); }}\n#[test]\nfn t() {{}}\n", exec_pat())).unwrap();
    // one sandbox-classified file so the fixture can fill SANDBOX_SHARDS=1
    fs::write(dir.join("tests/zz_sbx.rs"), format!("fn f() {{ require_user_namespaces_{}(); }}\n#[test]\nfn t() {{}}\n", "or_ci_skip")).unwrap();
    fs::write(dir.join("tests/suite_sandbox_01.rs"), "#[path = \"zz_sbx.rs\"]\nmod zz_sbx;\n").unwrap();
    fs::write(
        dir.join("tests/suite_core_01.rs"),
        "#[path = \"zz_spawn.rs\"]\nmod zz_spawn;\n#[path = \"zz_exec.rs\"]\nmod zz_exec;\n",
    )
    .unwrap();
    dir
}

#[test]
fn ctrx_ac03_check_fails_a_fixture_suite_mixing_one_of_each() {
    let dir = fixture();
    let out = Command::new(dir.join("scripts/gen-test-suites.sh"))
        .arg("--check")
        .env("SANDBOX_SHARDS", "1")
        .env("CORE_SHARDS", "1")
        .current_dir(&dir)
        .output()
        .unwrap();
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "{all}");
    let line = all.lines().find(|l| l.contains("zz_spawn.rs") && l.contains("zz_exec.rs")).unwrap_or_else(|| panic!("{all}"));
    assert!(line.contains("suite_core_01"), "{line}");
    let _ = fs::remove_dir_all(&dir);
}
