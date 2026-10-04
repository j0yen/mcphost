//! PRD-mcphost-oauth-conformance-harness
//! AC1 (P0) — Given an in-process host on this PRD's tree, When the
//! `oauthconf` gate test runs every scenario, Then the verdict table
//! equals `scenarios.toml` (data-driven per scenario, not hardcoded here --
//! `owner_prd == "unassigned"` reads gold-unsupported, every claimed
//! scenario reads gold-pass), and the run finishes under 20 s.
//!
//! PRD-mcphost-implicit-signup: `prm_root_discovery` (family `prm`) and
//! `step_up_403_without_as_metadata` (family `step_up`) both flipped from
//! gold-pass back to gold-unsupported -- both probes bootstrap on a root
//! `/mcp` call that now succeeds anonymously (implicit signup) instead of
//! 401ing, so this file's own assertions no longer assume `prm`/`iss` are
//! the two families that read pass; `iss` alone still does.

use std::time::Instant;

use crate::common;
use crate::oauthclient;
use common::TestServer;
use oauthclient::Verdict;

#[tokio::test]
async fn gate_verdict_table_matches_gold_within_20s() {
    let server = TestServer::start().await;
    let mcp_url = format!("{}/mcp", server.base_url);
    let http = reqwest::Client::new();

    let start = Instant::now();
    let results = oauthclient::run_all(&http, &mcp_url).await;
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs() < 20, "gate run took {elapsed:?}, must finish under 20s");

    let gold = oauthclient::load_scenarios();
    assert_eq!(results.len(), gold.len(), "every scenario in scenarios.toml must have a result");

    let mut families_checked = std::collections::BTreeSet::new();
    for scenario in &gold {
        let result = results
            .iter()
            .find(|r| r.name == scenario.name)
            .unwrap_or_else(|| panic!("missing result for scenario {}", scenario.name));

        let expected = match scenario.expected.as_str() {
            "pass" => Verdict::Pass,
            "unsupported" => Verdict::Unsupported,
            other => panic!("scenarios.toml: unknown expected verdict {other:?} for {}", scenario.name),
        };
        assert_eq!(
            result.verdict, expected,
            "scenario {} (family {}) got {:?}, want {:?} -- records: {:?}",
            scenario.name, scenario.family, result.verdict, expected, result.records
        );

        // requirement 2 goal 3: "every OAuth PRD in this fleet names the
        // scenarios it flips from unsupported to pass" via `owner_prd` --
        // an unclaimed scenario must read gold-unsupported, a claimed one
        // must read gold-pass, regardless of which family it belongs to
        // (PRD-mcphost-tool-scopes-and-consent AC3 is the first claim
        // beyond this PRD's own prm/iss).
        if scenario.owner_prd == "unassigned" {
            assert_eq!(expected, Verdict::Unsupported, "AC1: unclaimed scenario {} must be gold-unsupported", scenario.name);
        } else {
            assert_eq!(expected, Verdict::Pass, "AC1: scenario {} claimed by {} must be gold-pass", scenario.name, scenario.owner_prd);
        }
        families_checked.insert(scenario.family.clone());
    }

    assert!(families_checked.contains("prm"), "gold must cover the prm family");
    assert!(families_checked.contains("iss"), "gold must cover the iss family");
    assert!(
        families_checked.len() > 2,
        "gold must cover at least one family beyond prm/iss (got {families_checked:?})"
    );
}
