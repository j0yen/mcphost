//! PRD-mcphost-msrv-locked-required-check AC3 -- `required-checks.sh
//! --generate` derives exactly the 9 PR contexts from the workflow files
//! (matrix expanded, tag-only `publish` omitted) without naming any job.

use crate::msrvlock;
use msrvlock as support;

#[test]
fn msrvlock_ac03_generate_writes_exactly_nine_contexts() {
    let dir = support::scratch("ac03");
    let out_toml = dir.join("required-checks.toml");
    let o = support::run_script(
        &support::repo_root(),
        &["--generate"],
        &[("REQUIRED_CHECKS_TOML", out_toml.as_path())],
    );
    assert!(o.status.success(), "--generate failed:\n{}", support::text(&o));
    let got = support::toml_contexts(&std::fs::read_to_string(&out_toml).expect("read toml"));
    let mut want: Vec<String> = support::contexts();
    want.sort();
    assert_eq!(got, want, "generated contexts differ from the fixed contexts plus one per sandbox shard");
    assert!(
        !got.iter().any(|c| c.contains("publish to the MCP registry")),
        "tag-only `publish` job must be omitted: {got:?}"
    );

    // The committed file is the same list.
    let committed = support::toml_contexts(
        &std::fs::read_to_string(support::repo_root().join(".github/required-checks.toml"))
            .expect("read committed .github/required-checks.toml"),
    );
    assert_eq!(committed, want, "committed .github/required-checks.toml drifted");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn msrvlock_ac03_script_names_no_job() {
    let script = std::fs::read_to_string(support::repo_root().join("scripts/required-checks.sh"))
        .expect("read script");
    let all = support::contexts();
    let mut names: Vec<&str> = all.iter().map(String::as_str).collect();
    names.push("publish to the MCP registry");
    names.push("determinism (two cold release builds");
    for n in names {
        assert!(!script.contains(n), "script must not hardcode job name {n:?}");
    }
    // Job ids from the workflow files, as `  <id>:` keys under `jobs:`.
    for e in std::fs::read_dir(support::repo_root().join(".github/workflows")).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        let mut in_jobs = false;
        for l in text.lines() {
            if !l.is_empty() && !l.starts_with(' ') && !l.starts_with('#') {
                in_jobs = l.starts_with("jobs:");
            } else if in_jobs && l.starts_with("  ") && !l.starts_with("   ") && l.ends_with(':') {
                let id = l.trim().trim_end_matches(':');
                assert!(
                    !script.contains(&format!("\"{id}\"")) && !script.contains(&format!("'{id}'")),
                    "script must not name job id {id:?}"
                );
            }
        }
    }
}
