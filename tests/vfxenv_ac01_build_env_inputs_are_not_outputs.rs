//! VFX flake class (a test binary deleted under a running suite) AC1 -- Given
//! `build.rs`, When its `cargo:rerun-if-env-changed=<NAME>` inputs and its
//! `cargo:rustc-env=<NAME>=...` outputs are collected, Then no NAME is in
//! both sets.
//!
//! Why: cargo exports a package's `rustc-env` values into the environment of
//! the test processes it runs. A name that is also a `rerun-if-env-changed`
//! input is therefore "unset" at the outer build and "set" inside the suite,
//! so the nested `cargo test` that `tlint_ac06` runs reruns `build.rs`,
//! rebuilds the lib and relinks `deps/suite_*` plus `target/debug/mcphost`
//! under the running outer suite (`current_exe()` then reads NotFound). The
//! sets are read out of `build.rs` itself, so a new overlap cannot be added
//! without this test naming it.

use std::collections::BTreeSet;

/// Every NAME that follows `marker` on a non-comment line of `src`.
fn names_after(src: &str, marker: &str, terminator: fn(char) -> bool) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in src.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find(marker) {
            rest = &rest[at + marker.len()..];
            let name: String = rest.chars().take_while(|c| !terminator(*c)).collect();
            if !name.is_empty() {
                out.insert(name);
            }
        }
    }
    out
}

fn is_name_end(c: char) -> bool {
    !(c.is_ascii_alphanumeric() || c == '_')
}

/// (inputs, outputs, overlap) of one build-script source.
fn env_sets(src: &str) -> (BTreeSet<String>, BTreeSet<String>, Vec<String>) {
    let inputs = names_after(src, "rerun-if-env-changed=", is_name_end);
    let outputs = names_after(src, "rustc-env=", is_name_end);
    let overlap = inputs.intersection(&outputs).cloned().collect();
    (inputs, outputs, overlap)
}

#[test]
fn build_rs_env_inputs_and_rustc_env_outputs_are_disjoint() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs"))
        .expect("read build.rs");
    let (inputs, outputs, overlap) = env_sets(&src);
    assert!(!inputs.is_empty(), "parser found no rerun-if-env-changed input in build.rs");
    assert!(!outputs.is_empty(), "parser found no rustc-env output in build.rs");
    assert!(
        overlap.is_empty(),
        "build.rs names {overlap:?} as BOTH a `cargo:rerun-if-env-changed` input and a \
         `cargo:rustc-env` output; cargo exports the output into every test process, so a \
         nested `cargo test` sees the input \"changed\", reruns build.rs and relinks the \
         running suite's binaries. Emit it under a different name (inputs: {inputs:?}, \
         outputs: {outputs:?})"
    );
}

/// The parser itself: an overlap is found, a disjoint pair is not, comments
/// are ignored.
#[test]
fn overlap_detector_flags_a_shared_name_and_ignores_comments() {
    let bad = r#"
        println!("cargo:rerun-if-env-changed=MCPHOST_GIT_SHA");
        println!("cargo:rustc-env=MCPHOST_GIT_SHA={sha}");
    "#;
    assert_eq!(env_sets(bad).2, vec!["MCPHOST_GIT_SHA".to_string()]);

    let good = r#"
        // cargo:rustc-env=MCPHOST_GIT_SHA=never-emitted-in-a-comment
        println!("cargo:rerun-if-env-changed=MCPHOST_GIT_SHA");
        println!("cargo:rustc-env=MCPHOST_BUILD_GIT_SHA={sha}");
    "#;
    let (inputs, outputs, overlap) = env_sets(good);
    assert!(overlap.is_empty(), "{overlap:?}");
    assert_eq!(inputs.into_iter().collect::<Vec<_>>(), vec!["MCPHOST_GIT_SHA".to_string()]);
    assert_eq!(outputs.into_iter().collect::<Vec<_>>(), vec!["MCPHOST_BUILD_GIT_SHA".to_string()]);
}
