//! PRD-mcphost-registry-entry-one-source AC3 (P0) -- Given the repo tree,
//! When the one-source test runs, Then `git ls-files` contains exactly one
//! `server.json`, `deploy/listings/manifest.yaml`'s `registry.artifact`
//! equals `MANIFEST_PATH`, and `.github/workflows/registry.yml` contains no
//! bare `server.json` path.
//!
//! Text-level YAML reading (no YAML crate in this repo, same call as
//! `reglist_ac04`): the `registry` row is the block starting at
//! `- id: registry` up to the next `- id:`.

use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn regone_ac03() {
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(root())
        .output()
        .expect("git ls-files");
    let listing = String::from_utf8_lossy(&out.stdout);
    let found: Vec<&str> = listing
        .lines()
        .filter(|l| l.rsplit('/').next() == Some("server.json"))
        .collect();
    assert_eq!(found, vec![mcphost::registry_manifest::MANIFEST_PATH], "one server.json");

    let manifest = std::fs::read_to_string(root().join("deploy/listings/manifest.yaml")).unwrap();
    let start = manifest.find("- id: registry\n").expect("registry row");
    let block = &manifest[start..];
    let end = block[1..].find("\n- id:").map(|i| i + 1).unwrap_or(block.len());
    let artifact = block[..end]
        .lines()
        .find_map(|l| l.trim().strip_prefix("artifact:"))
        .expect("registry.artifact")
        .trim();
    assert_eq!(artifact, mcphost::registry_manifest::MANIFEST_PATH);

    let workflow = std::fs::read_to_string(root().join(".github/workflows/registry.yml")).unwrap();
    for (i, line) in workflow.lines().enumerate() {
        let mut rest = line;
        while let Some(pos) = rest.find("server.json") {
            let prefix = &rest[..pos];
            assert!(
                prefix.ends_with("registry/"),
                "workflow line {} has a bare server.json path: {line}",
                i + 1
            );
            rest = &rest[pos + "server.json".len()..];
        }
    }
}
