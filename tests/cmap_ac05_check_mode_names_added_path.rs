//! PRD-mcphost-generated-code-map
//! AC5 — Given a fixture copy with one source file added after generation, When
//! `scripts/gen-codemap.sh --check` runs, Then it exits 1 naming the added path;
//! on a freshly generated copy it exits 0; and ci.yml's `regen-check` job
//! invokes `scripts/gen-codemap.sh --check`.

use crate::codemap;

#[test]
fn cmap_ac05_check_exits_1_naming_the_added_path_and_0_when_fresh() {
    let root = codemap::fixture(
        "ac05",
        &[("src/a.rs", codemap::src_file("A.")), ("src/b.rs", codemap::src_file("B."))],
    );
    assert!(codemap::run(&root, false).status.success());
    let fresh = codemap::run(&root, true);
    assert_eq!(fresh.status.code(), Some(0), "{}", String::from_utf8_lossy(&fresh.stderr));

    codemap::write(&root, "src/added.rs", &codemap::src_file("Added after generation."));
    codemap::git_stage(&root);
    let stale = codemap::run(&root, true);
    assert_eq!(stale.status.code(), Some(1), "stale map must exit exactly 1");
    let err = String::from_utf8_lossy(&stale.stderr);
    assert!(err.contains("src/added.rs"), "names the added path: {err}");
    assert!(
        !std::fs::read_to_string(root.join("CODEMAP.md")).expect("map").contains("added.rs"),
        "--check writes nothing"
    );
}

#[test]
fn cmap_ac05_ci_regen_check_job_invokes_the_check() {
    let ci = std::fs::read_to_string(codemap::repo().join(".github/workflows/ci.yml")).expect("ci.yml");
    let job: Vec<&str> = ci
        .lines()
        .skip_while(|l| *l != "  regen-check:")
        .enumerate()
        .take_while(|(i, l)| *i == 0 || !(l.starts_with("  ") && !l.starts_with("   ") && l.trim_end().ends_with(':')))
        .map(|(_, l)| l)
        .collect();
    assert!(!job.is_empty(), "regen-check job exists");
    assert!(
        job.iter().any(|l| l.trim() == "check ./scripts/gen-codemap.sh --check"),
        "regen-check must run scripts/gen-codemap.sh --check"
    );
}
