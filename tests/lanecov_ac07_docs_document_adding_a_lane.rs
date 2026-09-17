//! PRD-mcphost-proof-lane-loop-config, AC7 (P2) — the docs explain how to
//! add a lane when adding a new top-level path, and name `loop-config` as
//! the worked example.
//!
//! AC7 is written as a grep ("Given the docs after land, When grepped for
//! `loop-config`, Then the how-to-add-a-lane paragraph is present"). The
//! grep is what this test runs; making it a test is what stops the
//! paragraph from being deleted by a later docs regeneration without
//! anyone noticing.

use std::fs;
use std::path::Path;

/// The doc this PRD extended. `scripts/gen-agent-docs.sh` regenerates parts
/// of the agent docs, so pinning the assertion to a file makes a drop
/// visible.
const QUICKSTART: &str = "docs/agent-quickstart.md";

#[test]
fn lanecov_ac07_quickstart_documents_adding_a_lane() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = fs::read_to_string(dir.join(QUICKSTART))
        .unwrap_or_else(|e| panic!("{QUICKSTART} must exist and be readable: {e}"));

    assert!(
        text.contains("loop-config"),
        "{QUICKSTART} must name the loop-config lane as the worked example"
    );
    assert!(
        text.contains("agent/proof-lanes.toml"),
        "{QUICKSTART} must point at agent/proof-lanes.toml as the file to edit"
    );
    for key in ["globs", "required_commands"] {
        assert!(
            text.contains(key),
            "{QUICKSTART}'s how-to-add-a-lane paragraph must name the `{key}` field"
        );
    }
}
