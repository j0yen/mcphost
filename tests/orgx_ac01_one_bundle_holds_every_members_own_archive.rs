//! PRD-mcphost-org-export-and-offboard-v2
//! AC1 — Given an org of 3 members with 2 tools each, whose member keys the
//! test rotated and discarded, When the owner runs `host.org.export` and the
//! job completes, Then one signed `download_url` serves a tar.gz whose
//! `members/<ns>.tar.gz` parts list each member's own tools as `host.export`
//! does, `org_manifest.members` has 3 rows with matching `sha256`, and
//! `grep -c key_hash src/org_export.rs` prints 0.

use crate::common;
use common::orgx::{download_bundle, fixture_org, read_tar_gz, rotate_and_discard, run_org_export};
use common::TestServer;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn orgx_ac01() {
    let server = TestServer::start().await;
    // Owner + 2 joiners = the free plan's 3 seats.
    let fx = fixture_org(&server, "acme", 2, 2).await;
    for (_, member) in &fx.members {
        rotate_and_discard(member).await;
    }

    let result = run_org_export(&fx.owner).await;
    let bundle = download_bundle(&result).await;

    let manifest: Value = serde_json::from_slice(&bundle["org_manifest.json"]).expect("org_manifest.json");
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["org"], "acme");
    assert_eq!(result["org_manifest"], manifest, "runs.get carries the same org_manifest");
    let rows = manifest["members"].as_array().expect("members");
    assert_eq!(rows.len(), 3, "{manifest}");

    let mut tools_total = 0;
    for ns in fx.namespaces() {
        let row = rows.iter().find(|r| r["namespace"] == ns.as_str()).unwrap_or_else(|| panic!("row for {ns}: {manifest}"));
        let part_path = format!("members/{ns}.tar.gz");
        assert_eq!(row["part"], part_path.as_str());
        let part = &bundle[&part_path];
        assert_eq!(row["sha256"], format!("{:x}", Sha256::digest(part)).as_str(), "sha256 of {ns}'s part");
        assert_eq!(row["bytes"], part.len());

        // The part is the member's own host.export archive: its manifest.json lists its two tools.
        let entries = read_tar_gz(part);
        let inner: Value = serde_json::from_slice(&entries["manifest.json"]).expect("part manifest.json");
        let names: Vec<&str> = inner["tools"].as_array().expect("tools").iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), 2, "{ns}: {inner}");
        let listed: Vec<&str> = row["tools"].as_array().expect("row tools").iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(listed, names, "{ns}: org_manifest lists the part's own tools");
        for name in &names {
            assert!(entries.contains_key(&format!("tools/{name}/v1/config.json")), "{ns}: {name}");
        }
        tools_total += names.len();
    }
    assert_eq!(tools_total, 6);

    // The module never touches a credential column.
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/org_export.rs")).expect("read src/org_export.rs");
    assert_eq!(src.matches("key_hash").count(), 0, "grep -c key_hash src/org_export.rs must print 0");
}
