//! Shared core for the `lanecov_ac*` proofs (PRD-mcphost-proof-lane-loop-config).
//!
//! Every `tests/lanecov_ac<N>_*.rs` file `#[path]`-includes this module so
//! the per-AC proofs cannot disagree about what "routes" means: one
//! `Lane`/`LaneFile` schema, one glob compiler, one `find_unrouted`.
//!
//! Glob semantics deliberately mirror `agent/proof-lanes.toml`'s real
//! consumer: `autobuilder/autobuilder/src/vti_plan.rs` compiles each lane's
//! globs with the `globset` crate (`Glob::new` + `GlobSetBuilder`) over
//! `toml::from_str`-parsed lane specs. This module uses the same two crates
//! at the same major versions (see this crate's `Cargo.toml`) -- per the
//! PRD's Technical considerations note, if this matcher and vti-plan's ever
//! disagree, this code is wrong, not vti-plan.
//!
//! `allow(dead_code)`: each including test file uses only the part of this
//! core its own AC needs, so an unused helper here is expected, not debt.
#![allow(dead_code)]

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

/// One `[[lane]]` entry, deserialized straight from `agent/proof-lanes.toml`.
/// `note` is optional (only some lanes carry one) and otherwise unused here.
#[derive(Debug, Deserialize)]
pub struct Lane {
    pub id: String,
    #[allow(dead_code)]
    pub description: String,
    pub globs: Vec<String>,
    pub required_commands: Vec<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LaneFile {
    #[serde(rename = "lane", default)]
    pub lanes: Vec<Lane>,
}

/// Paths that intentionally route to no lane. Route a new entry with a real
/// lane in a follow-on PRD rather than growing this list further.
pub const UNROUTED_ALLOWLIST: &[&str] = &[
    // Repo-hygiene file, same conceptual class as .gitignore (which the
    // "meta" lane already covers) but not itself in that lane's globs.
    ".gitattributes",
    // Prose doc adjacent to docs/kinds/** and docs/receipts/** (which the
    // "docs" lane covers) but not itself in that lane's globs.
    "docs/audit-notes.md",
    // PRD-mcphost-wasm-kind: the wasm kind's test fixtures. The four
    // wasm-src/<name>/ directories are cargo-component guest crates built
    // out-of-band (via the remote build runner's `cargo component build`,
    // never by this crate's own `cargo build`/`cargo test`) into the
    // checked-in *.wasm binaries; both are read only by
    // tests/**/*.rs (already the "rust-tests" lane's own glob) via
    // `wasm_fixture_b64`/`include_bytes!`-adjacent helpers, gated by the
    // exact same `cargo test --workspace` proof, but neither directory
    // itself matches `tests/**/*.rs` (no `.rs` extension) or any other
    // lane's globs. agent/proof-lanes.toml is read-only for the edit-agent
    // (this file's own header comment), so a real lane for
    // `tests/fixtures/**` is left to a follow-on PRD rather than attempted
    // here.
    "tests/fixtures/wasm-src/echo/Cargo.toml",
    "tests/fixtures/wasm-src/echo/wit/world.wit",
    "tests/fixtures/wasm-src/trap/Cargo.toml",
    "tests/fixtures/wasm-src/trap/wit/world.wit",
    "tests/fixtures/wasm-src/loop_forever/Cargo.toml",
    "tests/fixtures/wasm-src/loop_forever/wit/world.wit",
    "tests/fixtures/wasm-src/oom/Cargo.toml",
    "tests/fixtures/wasm-src/oom/wit/world.wit",
    "tests/fixtures/wasm/echo.wasm",
    "tests/fixtures/wasm/trap.wasm",
    "tests/fixtures/wasm/loop_forever.wasm",
    "tests/fixtures/wasm/oom.wasm",
];

/// Compiles one lane's globs into a matchable set, or an error naming the
/// lane id and the bad glob (P1 requirement 4 / AC6's "malformed glob" half).
pub fn compile_lane_globset(lane: &Lane) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for g in &lane.globs {
        let glob =
            Glob::new(g).map_err(|e| format!("lane {} has invalid glob {g:?}: {e}", lane.id))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| format!("lane {} failed to compile its globset: {e}", lane.id))
}

/// P1 requirement 4: every lane's `required_commands` must be non-empty and
/// every lane's globs must compile, so a malformed lane fails these tests
/// instead of silently never matching or reporting confidence 0 at vti-plan
/// time. Returns the first error, naming the lane id.
pub fn validate_lane_shapes(lanes: &[Lane]) -> Result<Vec<GlobSet>, String> {
    let mut sets = Vec::with_capacity(lanes.len());
    for lane in lanes {
        if lane.required_commands.is_empty() {
            return Err(format!("lane {} has empty required_commands", lane.id));
        }
        sets.push(compile_lane_globset(lane)?);
    }
    Ok(sets)
}

/// Returns every path in `paths` that no `globset` in `lane_sets` matches,
/// excluding anything in `allowlist`.
pub fn find_unrouted<'a>(
    lane_sets: &[GlobSet],
    paths: &'a [String],
    allowlist: &[&str],
) -> Vec<&'a str> {
    paths
        .iter()
        .map(String::as_str)
        .filter(|p| !allowlist.contains(p))
        .filter(|p| !lane_sets.iter().any(|set| set.is_match(p)))
        .collect()
}

pub fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

pub fn load_lane_file(manifest_dir: &Path) -> LaneFile {
    let text = std::fs::read_to_string(manifest_dir.join("agent/proof-lanes.toml"))
        .expect("agent/proof-lanes.toml must exist");
    toml::from_str(&text)
        .expect("agent/proof-lanes.toml must be valid TOML matching the Lane schema")
}

/// Reads a `[[lane]]` file out of a git revision (e.g. a tag) without
/// touching the working tree. `Ok(None)` when the revision or the path is
/// unknown to this checkout (a shallow clone, or a gate producer's rsynced
/// sandbox with no `.git`) -- the only honest answer there is "cannot check
/// here", never a false pass.
pub fn load_lane_file_at_rev(manifest_dir: &Path, rev: &str) -> Option<LaneFile> {
    let output = Command::new("git")
        .arg("show")
        .arg(format!("{rev}:agent/proof-lanes.toml"))
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    toml::from_str(&text).ok()
}

/// `Ok(None)` means this checkout has no `.git` to ask at all (observed in
/// the wild: a burst-lane gate producer runs against an rsynced copy of the
/// worktree with `--exclude .git`, per that tool's own sync contract --
/// there is no tracked-file list to prove coverage against there). Any OTHER
/// git failure is still a hard error.
pub fn git_ls_files(manifest_dir: &Path) -> Result<Option<Vec<String>>, String> {
    let output = Command::new("git")
        .arg("ls-files")
        .current_dir(manifest_dir)
        .output()
        .map_err(|e| format!("git ls-files could not be run: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Ok(None);
        }
        return Err(format!("git ls-files exited non-zero: {stderr}"));
    }
    let paths = String::from_utf8(output.stdout)
        .map_err(|e| format!("git ls-files output must be UTF-8: {e}"))?
        .lines()
        .map(str::to_owned)
        .filter(|l| !l.is_empty())
        .collect();
    Ok(Some(paths))
}
