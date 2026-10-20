//! PRD-mcphost-msrv-locked-required-check AC5 -- the committed
//! `.github/required-checks.toml` equals what `--generate` derives from the
//! workflow files, so a new/renamed job or a matrix change fails here, not
//! silently on GitHub.

use crate::msrvlock;
use msrvlock as support;

fn committed_toml() -> std::path::PathBuf {
    support::repo_root().join(".github/required-checks.toml")
}

#[test]
fn msrvlock_ac05_committed_toml_equals_generate() {
    let wf = support::repo_root().join(".github/workflows");
    if let Err(diff) = support::toml_equals_generate(&wf, &committed_toml()) {
        panic!("required-checks.toml is stale; run scripts/required-checks.sh --generate:\n{diff}");
    }
}

#[test]
fn msrvlock_ac05_renamed_job_fails_naming_the_context_then_regenerates_clean() {
    let wf = support::copy_workflows("ac05a");
    let ci = wf.join("ci.yml");
    let text = std::fs::read_to_string(&ci).unwrap();
    let renamed = text.replace("name: docs external links resolve", "name: docs links resolve");
    assert_ne!(text, renamed, "fixture rename must change ci.yml");
    std::fs::write(&ci, renamed).unwrap();

    let err = support::toml_equals_generate(&wf, &committed_toml()).unwrap_err();
    assert!(err.contains("docs external links resolve"), "{err}");
    assert!(err.contains("docs links resolve"), "{err}");

    let regen = wf.parent().unwrap().join("regen.toml");
    let o = support::run_script(
        &support::repo_root(),
        &["--generate"],
        &[("WORKFLOWS_DIR", wf.as_path()), ("REQUIRED_CHECKS_TOML", regen.as_path())],
    );
    assert!(o.status.success(), "{}", support::text(&o));
    support::toml_equals_generate(&wf, &regen).expect("regenerated toml must match");
    let _ = std::fs::remove_dir_all(wf.parent().unwrap());
}

#[test]
fn msrvlock_ac05_matrix_gaining_a_shard_fails_naming_the_new_context() {
    let wf = support::copy_workflows("ac05b");
    let ci = wf.join("ci.yml");
    let text = std::fs::read_to_string(&ci).unwrap();
    // The matrix is derived from the plan job; pin a wider literal one here.
    let n = support::sandbox_shards();
    let wider: Vec<String> = (1..=n + 1).map(|i| i.to_string()).collect();
    let grown = text.replace(
        "matrix: ${{ fromJson(needs.plan.outputs.matrix) }}",
        &format!("matrix:\n        shard: [{}]", wider.join(", ")),
    );
    assert_ne!(text, grown, "fixture matrix change must change ci.yml");
    std::fs::write(&ci, grown).unwrap();

    let err = support::toml_equals_generate(&wf, &committed_toml()).unwrap_err();
    let new_context = format!("shard {}/{n}", n + 1);
    assert!(err.contains(&new_context), "{err}");
    let _ = std::fs::remove_dir_all(wf.parent().unwrap());
}
