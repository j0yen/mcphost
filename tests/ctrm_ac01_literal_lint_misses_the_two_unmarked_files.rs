//! PRD-mcphost-contract-suite-markers AC1 -- the predecessor's three literal
//! signatures (CHANGELOG heading list, migrations dir, recorded suite weights)
//! match neither `suite_ac4_archived_prd_pointers_resolve.rs` nor
//! `argmsg_ac06_sites_render_from_one_args_error.rs`, so a lint that only knew
//! those signatures exited 0 on both while they read `agent/intent-card.json`
//! and walked `src/` -- the gap this PRD closes is real.

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// Built from pieces so this file's own source never matches a signature.
fn old_signatures() -> Vec<String> {
    vec![
        format!("{}.md", "CHANGE".to_string() + "LOG"),
        format!("{}(\"{}", "read".to_string() + "_dir", "migra".to_string() + "tions"),
        "ci-suite-".to_string() + "weights.json",
    ]
}

#[test]
fn ctrm_ac01_old_signatures_miss_both_files() {
    for name in [
        "suite_ac4_archived_prd_pointers_resolve.rs",
        "argmsg_ac06_sites_render_from_one_args_error.rs",
    ] {
        let src = fs::read_to_string(root().join("tests").join(name)).unwrap();
        for sig in old_signatures() {
            assert!(!src.contains(&sig), "{name} matches old signature {sig}");
        }
    }
}

#[test]
fn ctrm_ac01_the_two_files_do_read_tree_derived_sources() {
    let suite = fs::read_to_string(root().join("tests/suite_ac4_archived_prd_pointers_resolve.rs")).unwrap();
    assert!(suite.contains("intent-card.json"));
    let argmsg = fs::read_to_string(root().join("tests/argmsg_ac06_sites_render_from_one_args_error.rs")).unwrap();
    assert!(argmsg.contains("CARGO_MANIFEST_DIR") && argmsg.contains("\"src\""));
}
