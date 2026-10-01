//! PRD-mcphost-chart-in-a-minute
//! AC1 — Given the vendored crates in `vendor/`, When the gate runs, Then
//! their own test suites pass unchanged and `vendor/VENDOR.md` names
//! source repo and commit.
//!
//! The "own test suites pass unchanged" half needs the root `Cargo.toml`'s
//! `[workspace]` table to actually list each `vendor/<crate>` as a member:
//! without it, a path dependency of a workspace-less root package is an
//! ordinary library dep whose `#[cfg(test)]` module is never compiled, and
//! `cargo test -p <crate>` errors "not found in workspace" instead of
//! running it -- and the gate's own no-filter `cargo test --workspace`
//! fallback lane (`.buildloop/ci-equivalent.toml`) silently never reaches
//! it either, since a non-member is invisible to `--workspace`.
//! `vendored_crates_test_suites_pass_under_the_gates_cargo_test` below
//! proves this directly by shelling out to `cargo test -p <crate>` for
//! each vendor crate and asserting it succeeds, so this file fails without
//! the workspace-membership fix rather than trusting a claim about it. The
//! remaining tests check the half only a running test binary can assert
//! statically: `VENDOR.md` names every crate's source repo and commit, and
//! each named crate is actually wired into `Cargo.toml` as a path
//! dependency (so "vendored, not just documented" holds).

const VENDOR_MD: &str = include_str!("../vendor/VENDOR.md");
const CARGO_TOML: &str = include_str!("../Cargo.toml");

const VENDORED_CRATES: [&str; 5] =
    ["mqo-chart-vocab", "mqo-result-profiler", "mqo-chart-recommender", "mqo-vega-emitter", "mqo-chart-caption"];

#[test]
fn vendor_md_names_source_repo_and_commit_for_every_crate() {
    for crate_name in VENDORED_CRATES {
        assert!(VENDOR_MD.contains(crate_name), "VENDOR.md must name {crate_name}: {VENDOR_MD}");
    }
    assert!(VENDOR_MD.contains("ai-stack"), "VENDOR.md must name the source repo: {VENDOR_MD}");
    assert!(VENDOR_MD.contains("4ef6c22"), "VENDOR.md must name the source commit: {VENDOR_MD}");
}

#[test]
fn every_vendored_crate_is_a_path_dependency_in_cargo_toml() {
    for crate_name in VENDORED_CRATES {
        assert!(
            CARGO_TOML.contains(&format!("vendor/{crate_name}")),
            "Cargo.toml must depend on vendor/{crate_name}: {CARGO_TOML}"
        );
    }
}

#[test]
fn every_vendored_crate_has_its_own_test_module() {
    for crate_name in VENDORED_CRATES {
        let lib_path = format!("vendor/{crate_name}/src/lib.rs");
        let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&lib_path))
            .unwrap_or_else(|e| panic!("read {lib_path}: {e}"));
        assert!(src.contains("#[cfg(test)]"), "{crate_name}'s src/lib.rs must keep its own test module: {lib_path}");
    }
}

#[test]
fn vendored_crates_test_suites_pass_under_the_gates_cargo_test() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    for crate_name in VENDORED_CRATES {
        let output = std::process::Command::new(&cargo)
            .args(["test", "-p", crate_name])
            .current_dir(manifest_dir)
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn `cargo test -p {crate_name}`: {e}"));
        assert!(
            output.status.success(),
            "`cargo test -p {crate_name}` must pass -- if it errors \"not found in workspace\" the \
             root Cargo.toml's [workspace] table is missing {crate_name} as a member, which also means \
             the gate's own no-filter `cargo test --workspace` run silently skips it:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
