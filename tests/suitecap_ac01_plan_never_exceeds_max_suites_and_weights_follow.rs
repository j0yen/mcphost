//! contract: reads scripts/gen-test-suites.sh and scripts/ci-test-partition.sh
//! The generated suite plan and the CI gate share two sets: the suite-count cap
//! (`MAX_SUITES`) and the per-suite weight table (`scripts/ci-suite-weights.json`).
//! Run 952's land (2026-10-18) hit both drifts: the generator emitted an 11th
//! suite and nothing generated its weight, so CI's `ci-test-partition.sh check`
//! refused a plan the generator had produced.
//!
//! This test runs the REAL `gen-test-suites.sh` / `ci-test-partition.sh` on a
//! synthetic tree whose caps are scaled down (the only edit: the two constant
//! lines) so the overflow needs 18 tiny files instead of ~2000, and asserts:
//!   1. one more core bucket than the cap allows is PACKED into the existing
//!      suites (<= MAX_SUITES core+sandbox suites), the weights file's keys equal
//!      the generated suite set (+ `lib`), and `ci-test-partition.sh check` passes;
//!   2. when no packing exists the generator FAILS ("cannot pack"), never
//!      emitting an over-cap plan;
//!   3. `gen-test-suites.sh --check` names a suite with no weight and a stale
//!      weight entry; measured weights survive a no-op regeneration.
//!   4. a regeneration that RESIZES a suite scales its measured seconds in
//!      `scripts/ci-suite-measured.json` with its recorded weight, so the stale
//!      check never compares the old member set's seconds with the new set's
//!      weight (ryzen7 run 404, 10-18: recorded 223 s vs measured 398 s refused
//!      the land precheck).
//!   5. every suite of the GENERATED set ends with a measured entry (run 1034,
//!      10-21: the land's `git checkout origin/main -- <measured> <weights>` handed the
//!      generator a measured file without the branch's suite_core_12, and `check`
//!      refused "unmeasured suite"): the branch's committed value (HEAD) when it has
//!      one, else the suite's weight listed under `_provisional`; a suite outside the
//!      generated set is never seeded, and a plan with nothing missing is untouched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The generator's own `MAX_PER_SUITE = ...` and `MAX_SUITES = N` lines, read from
/// the script (never a copy of their values here), so the scratch edit below
/// follows the real constants whenever they move.
fn constant_line(script: &str, prefix: &str) -> String {
    script
        .lines()
        .find(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("{prefix}... line missing from scripts/gen-test-suites.sh; update this test"))
        .to_string()
}

/// A scratch repo: 12 core files in 4 areas of 3, 6 sandbox files in 2 areas of
/// 3, caps scaled to 3 files per suite and `max_suites` core+sandbox suites,
/// 2 shards each. Uncapped that is 4 core + 2 sandbox = 6 suites.
fn scratch(max_suites: u32) -> PathBuf {
    let root = repo_root();
    let dir = std::env::temp_dir().join(format!(
        "mcphost-suitecap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::create_dir_all(dir.join("scripts")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(dir.join("scripts/ci-suite-weights.json"), "{\n  \"lib\": 5\n}\n").unwrap();
    fs::write(dir.join("scripts/ci-suite-measured.json"), "{}\n").unwrap();

    let script = fs::read_to_string(root.join("scripts/gen-test-suites.sh")).unwrap();
    let caps_line = constant_line(&script, "MAX_PER_SUITE = ");
    let max_line = constant_line(&script, "MAX_SUITES = ");
    let script = script
        .replace(&caps_line, r#"MAX_PER_SUITE = {"core": 3, "sandbox": 3}"#)
        .replace(&max_line, &format!("MAX_SUITES = {max_suites}"));
    fs::write(dir.join("scripts/gen-test-suites.sh"), script).unwrap();
    fs::copy(
        root.join("scripts/ci-test-partition.sh"),
        dir.join("scripts/ci-test-partition.sh"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for n in ["gen-test-suites.sh", "ci-test-partition.sh"] {
            let p = dir.join("scripts").join(n);
            let mut perm = fs::metadata(&p).unwrap().permissions();
            perm.set_mode(0o755);
            fs::set_permissions(&p, perm).unwrap();
        }
    }
    for area in ["ca", "cb", "cc", "cd"] {
        for i in 1..=3 {
            fs::write(
                dir.join(format!("tests/{area}_t{i}.rs")),
                "#[test]\nfn t() {}\n",
            )
            .unwrap();
        }
    }
    for area in ["sa", "sb"] {
        for i in 1..=3 {
            fs::write(
                dir.join(format!("tests/{area}_t{i}.rs")),
                concat!("// Python", "Kind marks this file sandbox-classified\n#[test]\nfn t() {}\n"),
            )
            .unwrap();
        }
    }
    dir
}

fn run(dir: &Path, script: &str, args: &[&str]) -> Output {
    Command::new(dir.join("scripts").join(script))
        .args(args)
        .current_dir(dir)
        .env("CORE_SHARDS", "2")
        .env("SANDBOX_SHARDS", "2")
        .output()
        .unwrap_or_else(|e| panic!("run {script}: {e}"))
}

fn text(o: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn plain_suites(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir.join("tests"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("suite_core_") || n.starts_with("suite_sandbox_"))
        .map(|n| n.trim_end_matches(".rs").to_string())
        .collect();
    v.sort();
    v
}

fn all_suites(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir.join("tests"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("suite_") && n.ends_with(".rs"))
        .map(|n| n.trim_end_matches(".rs").to_string())
        .collect();
    v.sort();
    v
}

fn weights(dir: &Path) -> serde_json::Map<String, serde_json::Value> {
    let t = fs::read_to_string(dir.join("scripts/ci-suite-weights.json")).unwrap();
    serde_json::from_str::<serde_json::Value>(&t)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

fn measured(dir: &Path) -> serde_json::Map<String, serde_json::Value> {
    let t = fs::read_to_string(dir.join("scripts/ci-suite-measured.json")).unwrap();
    serde_json::from_str::<serde_json::Value>(&t)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

fn write_json(path: &Path, m: &serde_json::Map<String, serde_json::Value>) {
    fs::write(path, serde_json::to_string_pretty(m).unwrap() + "\n").unwrap();
}

/// `run` with `GEN_SUITES_BRANCH_MEASURED` pointing at `branch` (the test-only stand-in for HEAD's copy).
fn run_with_branch(dir: &Path, branch: &Path) -> Output {
    Command::new(dir.join("scripts/gen-test-suites.sh"))
        .current_dir(dir)
        .env("CORE_SHARDS", "2")
        .env("SANDBOX_SHARDS", "2")
        .env("GEN_SUITES_BRANCH_MEASURED", branch)
        .output()
        .unwrap()
}

/// A planned scratch tree whose measured file looks like MAIN's copy after the land's
/// checkout: every generated suite measured at its weight except the returned victim.
/// Returns (dir, victim, victim's weight).
fn tree_with_a_suite_main_lacks() -> (PathBuf, String, f64) {
    let dir = scratch(4);
    assert!(run(&dir, "gen-test-suites.sh", &[]).status.success());
    let w = weights(&dir);
    let victim = all_suites(&dir).remove(0);
    let mut m = serde_json::Map::new();
    for k in all_suites(&dir) {
        if k != victim {
            m.insert(k.clone(), w[&k].clone());
        }
    }
    write_json(&dir.join("scripts/ci-suite-measured.json"), &m);
    let wv = w[&victim].as_f64().unwrap();
    (dir, victim, wv)
}

#[test]
fn a_generated_suite_main_lacks_is_seeded_from_the_branch_copy() {
    let (dir, victim, wv) = tree_with_a_suite_main_lacks();
    let real = (wv * 0.5 * 100.0).round() / 100.0;
    let branch = dir.join("branch-measured.json");
    let mut b = serde_json::Map::new();
    b.insert(victim.clone(), serde_json::json!(real));
    write_json(&branch, &b);

    let o = run_with_branch(&dir, &branch);
    assert!(o.status.success(), "{}", text(&o));
    let line = String::from_utf8_lossy(&o.stdout)
        .lines()
        .find(|l| l.starts_with(&format!("gen-test-suites: seeded measured {victim} = ")))
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("no seed line\n{}", text(&o)));
    assert!(line.ends_with("(src=head)"), "{line}");
    let m = measured(&dir);
    assert_eq!(m[&victim].as_f64().unwrap(), real, "the seed is the branch's value: {m:?}");
    assert!(m.get("_provisional").is_none(), "a real branch measurement is not provisional: {m:?}");
    let c = run(&dir, "ci-test-partition.sh", &["check"]);
    assert!(c.status.success(), "check must pass once the branch's value is seeded\n{}", text(&c));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn without_a_branch_value_the_weight_is_seeded_as_provisional_and_the_marker_survives_regen() {
    let (dir, victim, wv) = tree_with_a_suite_main_lacks();
    let branch = dir.join("branch-measured.json");
    fs::write(&branch, "{}\n").unwrap();

    let o = run_with_branch(&dir, &branch);
    assert!(o.status.success(), "{}", text(&o));
    assert!(
        String::from_utf8_lossy(&o.stdout).contains(&format!("gen-test-suites: seeded measured {victim} = ")),
        "{}",
        text(&o)
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("(src=weight-provisional)"), "{}", text(&o));
    let m = measured(&dir);
    assert_eq!(m[&victim].as_f64().unwrap(), wv, "the provisional seed is the computed weight: {m:?}");
    assert_eq!(m["_provisional"], serde_json::json!([victim]), "{m:?}");
    let c = run(&dir, "ci-test-partition.sh", &["check"]);
    assert!(c.status.success(), "check must pass on a provisional seed\n{}", text(&c));

    // A later regeneration (nothing missing now) neither re-seeds nor drops the marker.
    let before = fs::read_to_string(dir.join("scripts/ci-suite-measured.json")).unwrap();
    let again = run_with_branch(&dir, &branch);
    assert!(again.status.success(), "{}", text(&again));
    assert!(!String::from_utf8_lossy(&again.stdout).contains("seeded measured"), "{}", text(&again));
    assert_eq!(before, fs::read_to_string(dir.join("scripts/ci-suite-measured.json")).unwrap());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_suite_outside_the_generated_set_is_never_seeded() {
    let (dir, victim, wv) = tree_with_a_suite_main_lacks();
    let branch = dir.join("branch-measured.json");
    let mut b = serde_json::Map::new();
    b.insert("suite_core_99".into(), serde_json::json!(5.0));
    b.insert("lib".into(), serde_json::json!(7.0));
    b.insert(victim.clone(), serde_json::json!(wv * 0.5));
    write_json(&branch, &b);

    let o = run_with_branch(&dir, &branch);
    assert!(o.status.success(), "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(!out.contains("suite_core_99"), "{}", text(&o));
    assert_eq!(out.matches("seeded measured").count(), 1, "only the generated suite is seeded\n{}", text(&o));
    let m = measured(&dir);
    let suites = all_suites(&dir);
    for k in m.keys() {
        assert!(k == "_provisional" || suites.contains(k), "measured key {k} names no generated suite: {m:?}");
    }
    assert!(m.get("suite_core_99").is_none() && m.get("lib").is_none(), "{m:?}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_plan_with_nothing_missing_is_left_untouched() {
    let (dir, victim, wv) = tree_with_a_suite_main_lacks();
    let mut m = measured(&dir);
    m.insert(victim, serde_json::json!(wv)); // nothing missing now
    write_json(&dir.join("scripts/ci-suite-measured.json"), &m);
    let branch = dir.join("branch-measured.json");
    fs::write(&branch, "{\"suite_core_01\": 1234.5}\n").unwrap(); // would be read if anything were missing
    let before = fs::read_to_string(dir.join("scripts/ci-suite-measured.json")).unwrap();

    let o = run_with_branch(&dir, &branch);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        "gen-test-suites.sh: already up to date\n".to_string()
            + &format!("gen-test-suites.sh: {} suites over {} test files\n", all_suites(&dir).len(), 18),
        "no seed line, no write"
    );
    assert_eq!(before, fs::read_to_string(dir.join("scripts/ci-suite-measured.json")).unwrap());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_1034_replay_the_lands_checkout_of_mains_copy_keeps_the_branchs_measurement() {
    // The land's entry 1, on a real git tree: the branch commits a measured file that holds
    // `victim`; `git checkout origin/main -- <measured> <weights>` swaps in main's copies (no
    // `victim`) in the worktree and index while HEAD stays the branch; then gen-test-suites.sh runs.
    let dir = scratch(4);
    assert!(run(&dir, "gen-test-suites.sh", &[]).status.success());
    let w = weights(&dir);
    let victim = all_suites(&dir).remove(0);
    let wv = w[&victim].as_f64().unwrap();
    let real = (wv * 0.5 * 100.0).round() / 100.0;
    let mut branch = serde_json::Map::new();
    for k in all_suites(&dir) {
        branch.insert(k.clone(), if k == victim { serde_json::json!(real) } else { w[&k].clone() });
    }
    write_json(&dir.join("scripts/ci-suite-measured.json"), &branch);
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "branch"]);
    // main's copies, as the checkout leaves them (staged, HEAD untouched)
    let mut main_m = branch.clone();
    main_m.remove(&victim);
    write_json(&dir.join("scripts/ci-suite-measured.json"), &main_m);
    let mut main_w = w.clone();
    main_w.remove(&victim);
    write_json(&dir.join("scripts/ci-suite-weights.json"), &main_w);
    git(&["add", "scripts/ci-suite-measured.json", "scripts/ci-suite-weights.json"]);

    let o = run(&dir, "gen-test-suites.sh", &[]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains("(src=head)"), "{}", text(&o));
    assert_eq!(measured(&dir)[&victim].as_f64().unwrap(), real, "HEAD's measurement, not a weight");
    let c = run(&dir, "ci-test-partition.sh", &["check"]);
    assert!(c.status.success(), "the replayed land entry must leave check green\n{}", text(&c));
    fs::remove_dir_all(&dir).ok();
}
#[test]
fn an_overflowing_core_plan_is_packed_into_max_suites_with_weights() {
    let dir = scratch(4);
    let o = run(&dir, "gen-test-suites.sh", &[]);
    assert!(o.status.success(), "{}", text(&o));

    let plain = plain_suites(&dir);
    assert!(
        plain.len() <= 4,
        "uncapped this tree plans 6 core+sandbox suites; the generator must pack to <= 4, got {plain:?}"
    );

    let mut keys: Vec<String> = weights(&dir).keys().cloned().collect();
    keys.sort();
    let mut want = all_suites(&dir);
    want.push("lib".to_string());
    want.sort();
    assert_eq!(keys, want, "weights keys must equal the generated suites + lib");
    for (k, v) in weights(&dir) {
        assert!(v.as_f64().unwrap_or(0.0) >= 1.0, "weight {k} = {v}");
    }

    let c = run(&dir, "gen-test-suites.sh", &["--check"]);
    assert!(c.status.success(), "{}", text(&c));
    // A fresh plan has no measurements yet: the generator seeds each suite from its weight and lists
    // it as provisional, so `check` passes (no suite is ever unmeasured)...
    let seeded = run(&dir, "ci-test-partition.sh", &["check"]);
    assert!(seeded.status.success(), "a fresh plan seeds provisional entries, so check passes\n{}", text(&seeded));
    let fresh = measured(&dir);
    assert_eq!(
        fresh["_provisional"].as_array().unwrap().len(),
        all_suites(&dir).len(),
        "every fresh suite is provisional: {fresh:?}"
    );
    // ...while a required suite whose entry is simply missing is still named.
    let mut gap = fresh.clone();
    gap.remove("_provisional");
    let dropped = all_suites(&dir).remove(0);
    gap.remove(&dropped);
    fs::write(dir.join("scripts/ci-suite-measured.json"), serde_json::to_string_pretty(&gap).unwrap() + "\n").unwrap();
    let unmeasured = run(&dir, "ci-test-partition.sh", &["check"]);
    assert!(!unmeasured.status.success(), "a required suite with no measured entry must fail check\n{}", text(&unmeasured));
    assert!(String::from_utf8_lossy(&unmeasured.stderr).contains(&format!("unmeasured suite: {dropped} ")), "{}", text(&unmeasured));
    // ...and passes once a CI refresh has measured each required suite (the emitter's keys) at its recorded weight.
    let req = run(&dir, "ci-test-partition.sh", &["list", "measured-required"]);
    assert!(req.status.success(), "{}", text(&req));
    let w = weights(&dir);
    let mut m = serde_json::Map::new();
    for k in String::from_utf8_lossy(&req.stdout).lines() {
        m.insert(k.to_string(), w[k].clone());
    }
    assert_eq!(m.len(), all_suites(&dir).len(), "the emitter lists every generated suite and not lib");
    fs::write(dir.join("scripts/ci-suite-measured.json"), serde_json::to_string_pretty(&m).unwrap() + "\n").unwrap();
    let p = run(&dir, "ci-test-partition.sh", &["check"]);
    assert!(p.status.success(), "ci-test-partition.sh check refused the generated plan\n{}", text(&p));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_plan_that_cannot_fit_fails_instead_of_exceeding_the_cap() {
    let dir = scratch(2);
    let o = run(&dir, "gen-test-suites.sh", &[]);
    assert!(!o.status.success(), "an unpackable plan must not be emitted\n{}", text(&o));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("cannot pack"),
        "the failure must say why\n{}",
        text(&o)
    );
    assert!(plain_suites(&dir).is_empty(), "nothing may be written on failure");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_names_a_missing_and_a_stale_weight_and_measured_values_survive_regen() {
    let dir = scratch(4);
    assert!(run(&dir, "gen-test-suites.sh", &[]).status.success());

    // Measured values (as a CI refresh would record them) survive a regen.
    let mut w = weights(&dir);
    for (i, k) in w.clone().keys().enumerate() {
        w.insert(k.clone(), serde_json::json!(100 + i));
    }
    let wpath = dir.join("scripts/ci-suite-weights.json");
    fs::write(&wpath, serde_json::to_string_pretty(&w).unwrap() + "\n").unwrap();
    let before = fs::read_to_string(&wpath).unwrap();
    let again = run(&dir, "gen-test-suites.sh", &[]);
    assert!(again.status.success(), "{}", text(&again));
    assert_eq!(before, fs::read_to_string(&wpath).unwrap(), "a no-op regen must keep measured weights");

    // A weight missing for a generated suite, and one stale entry.
    let victim = plain_suites(&dir).remove(0);
    w.remove(&victim);
    w.insert("suite_core_99".to_string(), serde_json::json!(7));
    fs::write(&wpath, serde_json::to_string_pretty(&w).unwrap() + "\n").unwrap();
    let c = run(&dir, "gen-test-suites.sh", &["--check"]);
    assert!(!c.status.success(), "--check must fail on a weights/suites mismatch");
    let err = String::from_utf8_lossy(&c.stderr).into_owned();
    assert!(err.contains(&format!("no weight for suite {victim}")), "{err}");
    assert!(err.contains("stale entry for suite_core_99"), "{err}");

    // Regenerating repairs both.
    assert!(run(&dir, "gen-test-suites.sh", &[]).status.success());
    assert!(run(&dir, "gen-test-suites.sh", &["--check"]).status.success());
    assert!(weights(&dir).contains_key(&victim));
    assert!(!weights(&dir).contains_key("suite_core_99"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_resized_suite_scales_its_measured_seconds_with_its_weight() {
    let dir = scratch(4);
    assert!(run(&dir, "gen-test-suites.sh", &[]).status.success());

    // A CI refresh: every suite measured at exactly its recorded weight.
    let mut w = weights(&dir);
    let mut m = serde_json::Map::new();
    for k in all_suites(&dir) {
        w.insert(k.clone(), serde_json::json!(90));
        m.insert(k, serde_json::json!(90.0));
    }
    fs::write(dir.join("scripts/ci-suite-weights.json"), serde_json::to_string_pretty(&w).unwrap() + "\n").unwrap();
    let mpath = dir.join("scripts/ci-suite-measured.json");
    fs::write(&mpath, serde_json::to_string_pretty(&m).unwrap() + "\n").unwrap();
    let p = run(&dir, "ci-test-partition.sh", &["check"]);
    assert!(p.status.success(), "{}", text(&p));

    // Two of area ca's three files go away: the suite holding them shrinks.
    fs::remove_file(dir.join("tests/ca_t1.rs")).unwrap();
    fs::remove_file(dir.join("tests/ca_t2.rs")).unwrap();
    let o = run(&dir, "gen-test-suites.sh", &[]);
    assert!(o.status.success(), "{}", text(&o));

    let measured: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(&mpath).unwrap()).unwrap();
    assert!(
        measured.values().any(|v| v.as_f64().unwrap() < 90.0),
        "the shrunk suite's measured seconds must scale down with its weight: {measured:?}"
    );
    let suites = all_suites(&dir);
    for k in measured.keys() {
        assert!(suites.contains(k), "measured entry {k} names no generated suite");
    }
    let p = run(&dir, "ci-test-partition.sh", &["check"]);
    assert!(p.status.success(), "a re-pack must not make the weights stale\n{}", text(&p));
    fs::remove_dir_all(&dir).ok();
}
