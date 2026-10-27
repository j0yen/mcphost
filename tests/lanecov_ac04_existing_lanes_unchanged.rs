//! contract: the pre-existing agent/proof-lanes.toml lanes are unchanged
//! PRD-mcphost-proof-lane-loop-config, AC4 — the pre-existing lanes are not
//! disturbed by this PRD's addition.
//!
//! Scope note, stated plainly rather than left implicit: AC4 was drafted as
//! "diff `agent/proof-lanes.toml` against v0.54.1 and the ONLY change is the
//! added `loop-config` block". That byte-exact form stopped being true on
//! 2026-09-17 and NOT because of this PRD: PRD-mcphost-gate-debt-4f1112d's
//! commit 7094d50 also widened the pre-existing `meta` lane's globs to
//! include `.buildloop/**`, folding the same path into a second lane. That
//! is a different PRD's deliberate, landed change to the same file;
//! reverting it to restore a byte-exact diff would be this PRD overreaching
//! into another's scope, and the operational goal (`.buildloop/**` routes at
//! confidence 1.0) holds either way.
//!
//! What this file asserts instead is the PRD's own stated Goal and Non-goal
//! -- "No change to what any existing lane requires" -- as a live check
//! against the v0.54.1 baseline, plus "loop-config is the only lane this PRD
//! added". A future PRD that quietly rewrites an existing lane's
//! `required_commands` still fails here.

use crate::lanecov;

use lanecov::{load_lane_file, load_lane_file_at_rev, manifest_dir};

/// The tag this PRD's rollback base is pinned to (PRD "Technical
/// considerations": "The rollback base stays v0.54.1").
const BASELINE_REV: &str = "v0.54.1";

/// AC4: every lane that existed at the baseline still exists, with
/// byte-identical `required_commands`.
#[test]
fn lanecov_ac04_baseline_lane_requirements_unchanged() {
    let dir = manifest_dir();
    let Some(baseline) = load_lane_file_at_rev(dir, BASELINE_REV) else {
        println!(
            "lanecov_ac04_baseline_lane_requirements_unchanged: skipped — {BASELINE_REV} is not \
             resolvable in this checkout (e.g. a gate producer's rsynced sandbox with no .git); \
             this test runs for real in any checkout that has the tag"
        );
        return;
    };
    let head = load_lane_file(dir);

    let mut drifted: Vec<String> = Vec::new();
    for old in &baseline.lanes {
        match head.lanes.iter().find(|l| l.id == old.id) {
            None => drifted.push(format!("lane {} was removed since {BASELINE_REV}", old.id)),
            Some(new) if new.required_commands != old.required_commands => drifted.push(format!(
                "lane {}'s required_commands changed since {BASELINE_REV}: {:?} -> {:?}",
                old.id, old.required_commands, new.required_commands
            )),
            Some(_) => {}
        }
    }
    assert!(
        drifted.is_empty(),
        "this PRD must not change what any pre-existing lane requires: {drifted:?}"
    );
}

/// AC4's other half: `loop-config` is the only lane id added since the
/// baseline. A second unexplained lane appearing here means some change
/// rode in on this PRD's diff.
#[test]
fn lanecov_ac04_loop_config_is_the_only_added_lane() {
    let dir = manifest_dir();
    let Some(baseline) = load_lane_file_at_rev(dir, BASELINE_REV) else {
        println!(
            "lanecov_ac04_loop_config_is_the_only_added_lane: skipped — {BASELINE_REV} is not \
             resolvable in this checkout"
        );
        return;
    };
    let head = load_lane_file(dir);

    let mut added: Vec<&str> = head
        .lanes
        .iter()
        .map(|l| l.id.as_str())
        .filter(|id| !baseline.lanes.iter().any(|o| o.id == *id))
        .collect();
    added.sort_unstable();
    // PRD-mcphost-share-a-tool-not-a-key added its own "examples" lane
    // (routes examples/share-a-tool/** to the AC0x proof tests) -- a
    // second, intended addition since the baseline, not drift.
    // PRD-mcphost-wasm-kind added its own "wasm-fixtures" lane (routes
    // tests/fixtures/wasm-src/** and tests/fixtures/wasm/** to the
    // wasmkind_ac0x proof tests) -- a third, intended addition since the
    // baseline, not drift.
    // PRD-mcphost-host-tool-deprecation added its own "contracts" lane
    // (routes contracts/** to the mcphost_host_tool_deprecation_ac0x proof
    // tests) -- a fourth, intended addition since the baseline, not drift.
    // PRD-mcphost-checkcompat-port-race added its own "checkcompat-race-soak"
    // lane (routes scripts/checkcompat-race-soak.sh to AC5's stress-test
    // proof) -- a fifth, intended addition since the baseline, not drift.
    // PRD-mcphost-claude-code-plugin-and-snippets added its own "plugin"
    // lane (routes plugin/** and tests/plugin_assets.sh to the AC1
    // asset-check proof) -- a sixth, intended addition since the
    // baseline, not drift.
    // PRD-mcphost-admin-schema-contract added its own "admin-schemas" lane
    // (routes schemas/** to the mcphost_admin_schema_contract_ac0x proof
    // tests) -- a seventh, intended addition since the baseline, not drift.
    // PRD-mcphost-docs-semantic-search added its own "docsearch-fixtures"
    // lane (routes tests/fixtures/docsearch/** to the docsearch_ac09 proof
    // test) -- an eighth, intended addition since the baseline, not drift.
    // PRD-mcphost-shared-tool-call-path added its own "sharing-docs" lane
    // (routes docs/sharing.md and scripts/gen-docs-sharing.sh to that
    // script's own --check, kept separate from the pre-existing docs lane
    // specifically so THIS test's own required_commands-unchanged half
    // stays green) -- a ninth, intended addition since the baseline, not
    // drift.
    // PRD-mcphost-oauth-conformance-harness added its own "oauthconf-data"
    // lane (routes tests/oauthconf/** and docs/oauth-scenarios.md to the
    // oauthconf_ac0x proof tests and the doc-sharing check) -- a tenth,
    // intended addition since the baseline, not drift.
    // PRD-mcphost-chart-in-a-minute added its own "vendor" lane (routes
    // vendor/** -- the vendored ai-stack chart chain, AC1 -- to the
    // existing rust-source lane's own cargo check/clippy/test commands,
    // which Cargo's implicit-workspace rule already runs those crates
    // through) -- an eleventh, intended addition since the baseline, not
    // drift.
    // PRD-mcphost-test-suite-flake-lints added its own "flake-lint" lane
    // (routes scripts/flake-lint.sh and tests/flake-lint-allow.txt to a
    // name-filtered cargo test run over the flakelint_ac0N proof tests) --
    // a twelfth, intended addition since the baseline, not drift.
    // chore/listings-wave1 added its own "launch-docs" lane (routes
    // docs/launch/** -- the wave-1 directory-listing checklist -- to
    // scripts/launch-docs-check.sh, duplicated from the pre-existing docs
    // lane's own UTF-8/non-empty check rather than folded into it, for the
    // same required_commands-immutability reason the sharing-docs lane
    // above gives) -- a thirteenth, intended addition since the baseline,
    // not drift.
    // PRD-mcphost-spec-unknown-field-rejection added its own
    // "spec-fields-doc-check" lane (routes the new
    // scripts/spec-fields-doc-check.sh to its own --run, duplicated from
    // the pre-existing docs lane rather than folded into it, for the same
    // required_commands-immutability reason the sharing-docs/launch-docs
    // lanes above give) -- a fourteenth, intended addition since the
    // baseline, not drift.
    // PRD-mcphost-sandbox-bridge-discoverability added its own
    // "sandbox-api-docs" lane (routes scripts/sandbox-api-doc-check.sh --
    // the drift check that docs/kinds/python.md, www/llms.txt, and
    // plugin/skills/mcphost/SKILL.md stay in sync with
    // kinds::python::BRIDGE_MODULES -- to that script's own check mode,
    // kept separate from the pre-existing docs/www/plugin lanes for the
    // same required_commands-immutability reason the sharing-docs/
    // launch-docs lanes above already give) -- a fifteenth, intended
    // addition since the baseline, not drift.
    // PRD-mcphost-tool-naming-convention-and-aliases added its own
    // "tool-naming" lane (routes docs/tool-naming.md, docs/tools.md,
    // scripts/tool-naming-lint.sh, and scripts/gen-docs-tools.sh to the
    // lint's own --run, duplicated from the pre-existing docs lane rather
    // than folded into it, for the same required_commands-immutability
    // reason the sharing-docs/launch-docs/spec-fields-doc-check lanes
    // above give) -- a sixteenth, intended addition since the baseline,
    // not drift.
    // PRD-mcphost-status-host-pressure added its own "host-pressure-doc-check"
    // lane (routes scripts/host-pressure-doc-check.sh to its own --check, kept
    // separate from the docs lane for the same required_commands-immutability
    // reason as the lanes above) -- an intended addition, not drift.
    //
    // Rebasing mcphost-event-trigger-self-test onto
    // mcphost-tool-naming-convention-and-aliases (this rebase): that PRD
    // added its own "event-trigger-self-test-fixtures" lane (routes
    // tests/fixtures/event-trigger-self-test/** -- the real truth-tier
    // nightly artifacts AC11's live-check bar is exercised over -- to the
    // mcphost_event_trigger_self_test_ac11 proof tests) -- a seventeenth,
    // intended addition since the baseline, not drift.
    // PRD-mcphost-registry-listing added its own "registry-manifest" lane
    // (routes registry/** -- the committed, generator-written
    // registry/server.json and the pinned registry/schema.json -- to a
    // name-filtered cargo test run over the reglist_ac0N proof tests, kept
    // separate from the pre-existing `meta` lane -- which already covers
    // the unrelated root-level server.json from a different, tenant-facing
    // PRD -- for the same required_commands-immutability reason the
    // sharing-docs/launch-docs/tool-naming lanes above give) -- an
    // eighteenth, intended addition since the baseline, not drift.
    // PRD-mcphost-blob-virus-and-type-guard added its own
    // "blobguard-fixtures" lane (routes tests/fixtures/blobguard/** and the
    // generated docs/blob-types.md to the blobguard_ac0N proof tests and
    // gendocs's drift test) -- an intended addition, not drift.
    // PRD-mcphost-child-process-test-harness added its own "child-proc-lint"
    // lane (routes scripts/child-proc-lint.py and its fixture templates to
    // the script's own run over tests/ plus the cproc_ac tests, kept separate
    // from the pre-existing flake-lint lane for the same
    // required_commands-immutability reason as the lanes above) -- an
    // intended addition, not drift.
    // PRD-mcphost-msrv-locked-required-check added its own "required-checks"
    // lane (routes scripts/required-checks.sh and the generated
    // .github/required-checks.toml to its --print and the msrvlock proof
    // tests, kept separate from the ci lane for the same
    // required_commands-immutability reason as the lanes above) -- an
    // intended addition, not drift.
    // PRD-mcphost-example-manifest-contract added its own "example-manifest"
    // lane (routes the generated examples/index.json and
    // examples/example.schema.json to the exmf_ac proof tests, kept separate
    // from the examples lane for the same required_commands-immutability
    // reason as the lanes above) -- an intended addition, not drift.
    // PRD-mcphost-contract-output-schemas added its own "output-schemas"
    // lane (routes contracts/host-output-schemas.v1.json and the captured
    // response corpus tests/fixtures/outsch/** to the outsch_ac proof
    // tests) -- an intended addition since the baseline.
    // PRD-mcphost-golden-signals-self-probe added its own "heal-config" lane
    // (routes heal.toml -- the golden-signal thresholds compiled into
    // src/signals.rs via include_str! -- to the workspace check, clippy, and
    // test commands, so a change to it is proven the way a src/ edit is) --
    // an intended addition since the baseline.
    // PRD-mcphost-heal-action-registry-and-ledger added its own "heal-docs"
    // lane (routes scripts/heal-doc-check.sh and docs/self-healing.md to the
    // script's check mode) -- an intended addition since the baseline.
    // PRD-mcphost-elicitation-with-ask-fallback added its own "elicit-docs"
    // lane (routes the elicitation docs to their check) -- an intended
    // addition since the baseline (rb seat hand-fix 10-15 2:0x am, CI lanecov red on PR 280).
    // PRD-mcphost-task-handoff-live-script added its own "live-scripts" lane
    // (routes scripts/task-handoff-live.sh to its proof tests) -- an intended
    // addition since the baseline (rb seat hand-fix 10-15 5:5x am, CI lanecov red on PR 297).
    // PRD-mcphost-python-sdk-generated added its own "sdk-python" lane
    // (routes the generated sdk-python/** to its regenerate-and-diff check) --
    // an intended addition since the baseline, not drift.
    // PRD-mcphost-capabilities-generated-from-dispatch added its own
    // "capabilities-wire-check" lane (routes scripts/capabilities-wire-check.sh
    // to a proof lane) -- an intended addition since the baseline.
    // PRD-mcphost-audit-export-tamper-evident added its own "audit-export" lane
    // (routes scripts/audit-verify.py and its vector test to a proof lane) --
    // an intended addition since the baseline.
    // PRD-mcphost-dev-emulator added its own "dev-parity-live" lane (routes
    // scripts/dev-parity-prod.sh to the demu_ac proof tests) -- an intended
    // addition since the baseline.
    // hotfix signup-source-descriptor added its own "signup-args-doc-check"
    // lane (routes scripts/signup-args-doc-check.sh to its check mode) -- an
    // intended addition since the baseline.
    // PRD-mcphost-import-dry-run-live-real-export added its own
    // "osrx-live-script" lane (routes scripts/live/osreach_import_reach.py to
    // the osrx_ac proof tests) -- an intended addition since the baseline.
    // PRD-mcphost-table-import-url-live-proof added its own
    // "tblurlp-live-script" lane (routes scripts/live/tblurlp_import_live.py
    // to the tblurlp_ac03 proof tests) -- an intended addition since the
    // baseline.
    // PRD-mcphost-external-class-positive-evidence added its own
    // "origin-counts-script" lane (routes scripts/ops/origin-counts.py to the
    // AC6 and AC7 proof tests) -- an intended addition since the baseline, not
    // drift.
    assert_eq!(
        added,
        vec![
            "admin-schemas",
            "audit-export",
            "blobguard-fixtures",
            "capabilities-wire-check",
            "checkcompat-race-soak",
            "child-proc-lint",
            "codemap",
            "contracts",
            "dev-parity-live",
            "docs-links",
            "docsearch-fixtures",
            "elicit-docs",
            "event-trigger-self-test-fixtures",
            "example-manifest",
            "examples",
            "flake-lint",
            "heal-config",
            "heal-docs",
            "host-pressure-doc-check",
            "kind-contract-doc-check",
            "launch-docs",
            "live-scripts",
            "loop-config",
            "oauthconf-data",
            "openapi-doc",
            "origin-counts-script",
            "osrx-live-script",
            "output-schemas",
            "plugin",
            "recipes",
            "registry-manifest",
            "required-checks",
            "sandbox-api-docs",
            "sdk-python",
            "sharing-docs",
            "signup-args-doc-check",
            "spec-fields-doc-check",
            "tblurlp-live-script",
            "tool-naming",
            "vendor",
            "wasm-fixtures",
        ],
        "only admin-schemas, blobguard-fixtures, capabilities-wire-check, checkcompat-race-soak, child-proc-lint, contracts, docs-links, docsearch-fixtures, \
         event-trigger-self-test-fixtures, example-manifest, examples, flake-lint, heal-config, heal-docs, host-pressure-doc-check, launch-docs, \
         loop-config, oauthconf-data, origin-counts-script, osrx-live-script, output-schemas, plugin, registry-manifest, required-checks, sandbox-api-docs, sdk-python, sharing-docs, \
         spec-fields-doc-check, tblurlp-live-script, tool-naming, vendor, and wasm-fixtures may have been added since \
         {BASELINE_REV}"
    );
}
