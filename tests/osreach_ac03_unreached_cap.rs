//! contract: reads contracts/host-output-variants.v1.json
//! PRD-mcphost-output-schema-reach-sweep-live
//! AC3 (P0) -- Given `contracts/host-output-variants.v1.json` with a committed
//! `unreached_cap`, When the sweep runs, Then it prints the unreached count,
//! fails when the count exceeds the cap, and in write mode never writes a cap
//! above the committed one.

use crate::common::TempDataDir;
use crate::osreach;
use serde_json::json;

#[tokio::test]
async fn osreach_ac03_unreached_count_is_printed_and_within_the_committed_cap() {
    let sweep = osreach::sweep().await;
    let count = sweep.unreached_count() as u64;
    let cap = osreach::committed_cap(osreach::root()).expect("committed unreached_cap");
    println!("osreach unreached={count} cap={cap}");
    if let Err(e) = osreach::within_cap(count, cap) {
        panic!("{e}");
    }
}

#[test]
fn osreach_ac03_a_count_above_the_cap_fails() {
    let err = osreach::within_cap(11, 10).expect_err("over the cap");
    assert!(err.contains("11") && err.contains("unreached_cap 10"), "{err}");
    assert!(osreach::within_cap(10, 10).is_ok());
    assert!(osreach::within_cap(0, 10).is_ok());
}

#[test]
fn osreach_ac03_next_cap_never_rises() {
    assert_eq!(osreach::next_cap(None, 7), 7, "the first write sets the cap to the count");
    assert_eq!(osreach::next_cap(Some(10), 12), 10, "a higher count never raises the cap");
    assert_eq!(osreach::next_cap(Some(10), 4), 4, "a lower count lowers it");
}

#[tokio::test]
async fn osreach_ac03_write_mode_never_writes_a_cap_above_the_committed_one() {
    let sweep = osreach::sweep().await;
    let dir = TempDataDir::new();
    std::fs::create_dir_all(dir.0.join("contracts")).expect("contracts dir");
    let low = (sweep.unreached_count() as u64).saturating_sub(1);
    let record = json!({"unreached_cap": low, "tools": {}});
    std::fs::write(dir.0.join(osreach::VARIANTS_PATH), osreach::to_text(&record)).expect("seed");
    osreach::sync_variants(sweep, &dir.0, true).expect("write");
    assert_eq!(osreach::committed_cap(&dir.0), Some(low), "a count above the committed cap leaves the cap alone");
}
