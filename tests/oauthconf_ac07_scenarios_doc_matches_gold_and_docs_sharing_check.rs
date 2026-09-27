//! PRD-mcphost-oauth-conformance-harness
//! AC7 (P1) — Given `docs/oauth-scenarios.md`, When compared with
//! `scenarios.toml`, Then every scenario appears once with its family,
//! owner PRD and expected verdict, and `scripts/gen-docs-sharing.sh
//! --check` exits 0.

use std::collections::BTreeMap;
use std::process::Command;

use crate::oauthclient;

const OAUTH_SCENARIOS_DOC: &str = include_str!("../docs/oauth-scenarios.md");

struct DocRow {
    family: String,
    owner_prd: String,
    expected: String,
}

/// Parses the `| scenario | family | owner PRD | expected verdict |` table
/// `scripts/gen-oauth-scenarios-doc.sh` writes. Fails loudly (empty map) if
/// the table is missing entirely, rather than silently passing an equality
/// check against nothing.
fn parse_scenarios_doc(content: &str) -> BTreeMap<String, DocRow> {
    let mut rows = BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() != 4 || cells[0] == "scenario" || cells[0].chars().all(|c| c == '-' || c == ':') {
            continue;
        }
        rows.insert(
            cells[0].to_string(),
            DocRow { family: cells[1].to_string(), owner_prd: cells[2].to_string(), expected: cells[3].to_string() },
        );
    }
    rows
}

#[test]
fn every_gold_scenario_appears_once_with_matching_family_owner_and_verdict() {
    let doc_rows = parse_scenarios_doc(OAUTH_SCENARIOS_DOC);
    assert!(
        !doc_rows.is_empty(),
        "docs/oauth-scenarios.md must have a scenario table -- run scripts/gen-oauth-scenarios-doc.sh"
    );

    let gold = oauthclient::load_scenarios();
    assert_eq!(doc_rows.len(), gold.len(), "docs/oauth-scenarios.md must list exactly the gold scenarios, no more, no fewer");

    let mut seen = std::collections::BTreeSet::new();
    for scenario in &gold {
        assert!(
            seen.insert(scenario.name.clone()),
            "scenario {} appears more than once in tests/oauthconf/scenarios.toml itself",
            scenario.name
        );
        let row = doc_rows
            .get(&scenario.name)
            .unwrap_or_else(|| panic!("docs/oauth-scenarios.md is missing scenario {}", scenario.name));
        assert_eq!(row.family, scenario.family, "scenario {} family mismatch", scenario.name);
        assert_eq!(row.owner_prd, scenario.owner_prd, "scenario {} owner_prd mismatch", scenario.name);
        assert_eq!(row.expected, scenario.expected, "scenario {} expected verdict mismatch", scenario.name);
    }
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

#[test]
fn gen_oauth_scenarios_doc_check_exits_zero() {
    let status = Command::new(repo_root().join("scripts/gen-oauth-scenarios-doc.sh"))
        .arg("--check")
        .current_dir(repo_root())
        .status()
        .expect("spawn scripts/gen-oauth-scenarios-doc.sh");
    assert!(status.success(), "docs/oauth-scenarios.md must be exactly what the generator produces");
}

/// AC7's own text names this pre-existing, unrelated script (PRD-mcphost-
/// shared-tool-call-path's docs/sharing.md -> www/llms.txt block) --
/// nothing in this PRD touches its source or target, so this is a
/// regression guard: this PRD's own `www/llms.txt` edit (a new bullet in
/// `## Operator notes`, linking to `docs/oauth-scenarios.md`) must land
/// outside the `<!-- sharing:start -->`/`<!-- sharing:end -->` block.
#[test]
fn gen_docs_sharing_check_exits_zero() {
    let status = Command::new(repo_root().join("scripts/gen-docs-sharing.sh"))
        .arg("--check")
        .current_dir(repo_root())
        .status()
        .expect("spawn scripts/gen-docs-sharing.sh");
    assert!(status.success(), "scripts/gen-docs-sharing.sh --check must exit 0");
}
