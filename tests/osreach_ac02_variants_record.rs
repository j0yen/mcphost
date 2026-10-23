//! contract: writes contracts/host-output-variants.v1.json
//! contract: reads contracts/host-output-schemas.v1.json
//! contract: reads contracts/host-output-observational-edits.v1.json
//! PRD-mcphost-output-schema-reach-sweep-live
//! AC2 (P0) -- Given the sweep's rows, When it runs with `OSREACH_WRITE=1` and
//! then without, Then the first run rewrites only
//! `contracts/host-output-variants.v1.json`,
//! `contracts/host-output-schemas.v1.json` and
//! `contracts/host-output-observational-edits.v1.json` keep every byte, the
//! second run changes no byte, and a committed variants file that differs
//! fails the test naming the tool and JSON pointer.

use std::path::Path;

use crate::common::TempDataDir;
use crate::osreach;
use serde_json::{Value, json};

fn scratch_contracts() -> TempDataDir {
    let dir = TempDataDir::new();
    std::fs::create_dir_all(dir.0.join("contracts")).expect("contracts dir");
    for file in [osreach::SCHEMAS_PATH, osreach::EDITS_PATH] {
        std::fs::copy(osreach::root().join(file), dir.0.join(file)).expect("copy contract");
    }
    dir
}

fn bytes(dir: &Path, file: &str) -> Option<Vec<u8>> {
    std::fs::read(dir.join(file)).ok()
}

#[tokio::test]
async fn osreach_ac02_committed_variants_file_matches_the_sweep() {
    let sweep = osreach::sweep().await;
    if let Err(e) = osreach::sync_variants(sweep, osreach::root(), osreach::write_mode()) {
        panic!("{e}");
    }
}

#[tokio::test]
async fn osreach_ac02_write_mode_touches_only_the_variants_file_and_is_idempotent() {
    let sweep = osreach::sweep().await;
    let dir = scratch_contracts();
    let (schemas, edits) = (bytes(&dir.0, osreach::SCHEMAS_PATH), bytes(&dir.0, osreach::EDITS_PATH));
    assert!(bytes(&dir.0, osreach::VARIANTS_PATH).is_none());

    osreach::sync_variants(sweep, &dir.0, true).expect("first write");
    let first = bytes(&dir.0, osreach::VARIANTS_PATH).expect("variants file written");
    assert_eq!(bytes(&dir.0, osreach::SCHEMAS_PATH), schemas, "schemas keep every byte");
    assert_eq!(bytes(&dir.0, osreach::EDITS_PATH), edits, "observational edits keep every byte");

    osreach::sync_variants(sweep, &dir.0, true).expect("second write");
    assert_eq!(bytes(&dir.0, osreach::VARIANTS_PATH).expect("still there"), first, "the second run changes no byte");
    osreach::sync_variants(sweep, &dir.0, false).expect("compare mode passes on what write mode wrote");
    assert_eq!(bytes(&dir.0, osreach::SCHEMAS_PATH), schemas);
    assert_eq!(bytes(&dir.0, osreach::EDITS_PATH), edits);
}

#[tokio::test]
async fn osreach_ac02_a_differing_committed_file_names_the_tool_and_pointer() {
    let sweep = osreach::sweep().await;
    let dir = scratch_contracts();
    osreach::sync_variants(sweep, &dir.0, true).expect("write");
    let path = dir.0.join(osreach::VARIANTS_PATH);
    let mut record: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    let tool = sweep.rows.keys().next().expect("a tool").clone();
    record["tools"][tool.as_str()]["variants"] = json!(["planted"]);
    std::fs::write(&path, osreach::to_text(&record)).expect("plant");
    let err = osreach::sync_variants(sweep, &dir.0, false).expect_err("a differing file fails");
    assert!(err.contains(&format!("tool {tool} at /tools/{tool}/variants")), "{err}");
}
