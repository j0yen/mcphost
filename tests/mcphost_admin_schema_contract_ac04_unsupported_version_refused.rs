//! PRD-mcphost-admin-schema-contract
//! AC4 (P0) -- Given mcphost-deploy pinned to v1 and a listing reporting
//! `schema_version: 2`, When `measure` runs, Then it exits 5 with the
//! unsupported-version message and writes no `measure.json`.
//!
//! This AC straddles two repos, exactly as the PRD's "Technical
//! considerations" says it does ("the deploy tool copies them at build with
//! a recorded source commit"; "the mcphost-deploy change is small and is
//! listed in the trailer with its commit"). The two halves:
//!
//! * The *consumer* half -- `measure`'s exit 5, its message, and the
//!   absence of `measure.json` -- is Python, in mcphost-deploy. It is
//!   proved there by `tests/adminschema_ac4_unsupported_version_exit5.py`,
//!   which drives the real `measure` Typer command end to end and asserts
//!   the process exit code, the message and the missing file. The commit
//!   is recorded in `agent/test-map.json`'s AC4 entry, and
//!   `cross_repo_ac4_pointer_is_not_dangling` below keeps that pointer
//!   honest rather than decorative.
//! * The *producer* half is this repo's, and is what this file proves for
//!   real: the vendored v1 schemas -- the byte-for-byte files mcphost-deploy
//!   pins to -- accept a listing this branch actually serves and refuse one
//!   reporting `schema_version: 2`, naming `schema_version` in the error.
//!   That refusal is the condition mcphost-deploy's exit 5 fires on; without
//!   the `const` pin below, a v1-pinned consumer would happily measure a v2
//!   listing and there would be no exit 5 to test over there.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load_schema(rel: &str) -> Value {
    let path = repo_root().join(rel);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("read schema {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse schema {}: {e}", path.display()))
}

const VENDORED_SCHEMAS: [(&str, &str); 2] = [
    ("admin.tenants", "schemas/admin/tenants.v1.json"),
    ("admin.usage", "schemas/admin/usage.v1.json"),
];

/// The producer half of AC4: a v1-pinned validator accepts this branch's
/// real listings and refuses the same listing reporting `schema_version: 2`,
/// with an error naming the offending field. Both listings are fetched over
/// a real socket from a real server, so the "supported" side is this
/// branch's actual output rather than a hand-written fixture.
#[tokio::test]
async fn v1_pinned_validator_accepts_v1_and_refuses_a_listing_reporting_v2() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (_ns, _key) = signup(&server.base_url, "Unsupported Version Tenant").await;

    for (tool, schema_rel) in VENDORED_SCHEMAS {
        let validator =
            jsonschema::validator_for(&load_schema(schema_rel)).expect("compile vendored schema");

        // Given: mcphost-deploy pinned to v1 -- and a listing this branch
        // really serves.
        let listing = extract_structured(
            &admin
                .tools_call(tool, json!({}))
                .await
                .unwrap_or_else(|e| panic!("{tool}: {} {}", e.code, e.message)),
        );
        assert!(
            validator.is_valid(&listing),
            "{tool}'s live listing must satisfy the pinned v1 schema, otherwise the \
             refusal below proves nothing about the version: {listing}"
        );

        // When: the same listing reports schema_version: 2.
        let mut v2 = listing.clone();
        v2["schema_version"] = json!(2);

        // Then: the v1-pinned validator refuses it, naming schema_version.
        let errors: Vec<String> = validator
            .iter_errors(&v2)
            .map(|e| format!("{} at {}", e, e.instance_path))
            .collect();
        assert!(
            !errors.is_empty(),
            "{schema_rel} accepted a {tool} listing reporting schema_version: 2 -- a \
             consumer pinned to v1 would then measure a listing it cannot read, and \
             there would be no unsupported-version refusal to exit 5 on"
        );
        assert!(
            errors.iter().any(|e| e.contains("schema_version")),
            "the refusal must name schema_version (that is what the \
             unsupported-version message reports), got: {errors:?}"
        );
    }
}

/// The pin itself, stated once: `type: integer` alone would let
/// `schema_version: 2` through every validator that vendors these files.
#[test]
fn both_vendored_schemas_pin_schema_version_to_const_1() {
    for (tool, schema_rel) in VENDORED_SCHEMAS {
        let schema = load_schema(schema_rel);
        let pinned = &schema["properties"]["schema_version"]["const"];
        assert_eq!(
            pinned,
            &json!(1),
            "{schema_rel} ({tool}) must pin properties.schema_version.const to 1 so a \
             consumer that vendors it is pinned to v1 by construction, got {pinned}"
        );
    }
}

/// This PRD's own test_prefix (see agent/test-map.json's `ac_test_map_prd`),
/// used to scope the AC4 lookup below to mcphost-admin-schema-contract's
/// own entry and nothing else -- see `admin_schema_contract_ac4_entry`.
const PRD_MARKER: &str = "PRD-mcphost-admin-schema-contract.md";

/// wm-build run 189 gate block (2026-09-25, mcphost-document-store build,
/// flake-audit attempt 1): this test used to read `map["ac_test_map"]["AC4"]`
/// directly. `agent/test-map.json` documents `ac_test_map` as "the AC-to-test
/// mapping for the PRD CURRENTLY BUILDING AT HEAD -- not a repo-lifetime
/// constant" (its own `ac_test_map_contract` field) -- true only while THIS
/// PRD (mcphost-admin-schema-contract) is still the one at HEAD. As soon as
/// a later PRD build (mcphost-document-store, run 189) refreshed
/// `ac_test_map` to its own ACs, the same top-level key held a completely
/// different PRD's AC4 entry (its own ac04 test file,
/// `tests/docstore_ac04_extraction_json_csv.rs`), and this test -- which
/// belongs to mcphost-admin-schema-contract and must prove ITS OWN AC4
/// paper trail, not whichever PRD happens to be building -- panicked
/// comparing docstore's entry against the "must name mcphost-deploy"
/// assertion below. Fix: scope the lookup to this PRD's own test_prefix
/// (`mcphost_admin_schema_contract`) so no other PRD's ac04 file can ever
/// match: while this PRD is still current (`ac_test_map_prd` names it),
/// read the top-level `ac_test_map`; once superseded, read this PRD's
/// archived copy in `ac_test_map_by_prefix`, which `intent-card-refresh.sh`
/// preserves (keyed by `_prd`) precisely so a superseded PRD's own AC
/// mapping is never lost -- never by scanning for a loose `*ac04*` filename
/// pattern, which is exactly what let another PRD's file match here.
fn admin_schema_contract_ac4_entry(map: &Value) -> String {
    let ac_test_map_prd = map["ac_test_map_prd"]
        .as_str()
        .expect("agent/test-map.json ac_test_map_prd must be a string");
    if ac_test_map_prd.starts_with(PRD_MARKER) {
        return map["ac_test_map"]["AC4"]
            .as_str()
            .expect("agent/test-map.json ac_test_map.AC4 must be a string")
            .to_string();
    }

    let by_prefix = map["ac_test_map_by_prefix"]
        .as_object()
        .expect("agent/test-map.json ac_test_map_by_prefix must be an object");
    for entry in by_prefix.values() {
        let prd = entry.get("_prd").and_then(Value::as_str).unwrap_or("");
        if prd.starts_with(PRD_MARKER) {
            return entry
                .get("AC4")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "agent/test-map.json's ac_test_map_by_prefix entry for \
                         {PRD_MARKER} has no AC4 key: {entry}"
                    )
                })
                .to_string();
        }
    }
    panic!(
        "agent/test-map.json names neither ac_test_map_prd={ac_test_map_prd:?} nor any \
         ac_test_map_by_prefix entry as {PRD_MARKER} -- mcphost-admin-schema-contract's own \
         AC4 paper trail has been lost, not just superseded by a later PRD build"
    );
}

/// AC4's consumer half is cross-repo; this keeps the pointer to it from
/// rotting into the kind of dangling reference `checkcompat_race_ac08_*`
/// locks against: `agent/test-map.json`'s AC4 entry must name the repo, a
/// commit, and the Python test file -- and where that repo is present on
/// this machine, the named test file must really exist.
#[test]
fn cross_repo_ac4_pointer_is_not_dangling() {
    let path = repo_root().join("agent/test-map.json");
    let map: Value = serde_json::from_str(
        &fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
    let entry = admin_schema_contract_ac4_entry(&map);

    assert!(
        entry.contains("mcphost-deploy"),
        "AC4's entry must name the repo its consumer half landed in: {entry:?}"
    );
    let test_file = entry
        .split_whitespace()
        .find(|t| t.starts_with("tests/") && t.ends_with(".py"))
        .unwrap_or_else(|| {
            panic!("AC4's entry must name the mcphost-deploy test file proving it: {entry:?}")
        });
    let commit = entry
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|t| t.len() >= 7 && t.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| {
            panic!("AC4's entry must record the mcphost-deploy commit it landed as: {entry:?}")
        });
    assert!(
        commit.len() >= 7,
        "AC4's recorded commit must be at least a short sha: {commit:?}"
    );

    let repo = entry
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || "/-_.".contains(c))))
        .find(|t| t.starts_with('/') && t.ends_with("mcphost-deploy"))
        .unwrap_or_else(|| {
            panic!("AC4's entry must give the absolute path of the deploy repo: {entry:?}")
        });
    let repo_dir = Path::new(repo);
    if repo_dir.is_dir() {
        // Present on this machine (the operator's checkout): the pointer has
        // to resolve. Absent (the runner box, CI): nothing to check against,
        // and a missing sibling checkout is not an AC failure.
        assert!(
            repo_dir.join(test_file).is_file(),
            "{repo}/{test_file} does not exist, but agent/test-map.json's AC4 entry \
             claims it proves AC4"
        );
    }
}
