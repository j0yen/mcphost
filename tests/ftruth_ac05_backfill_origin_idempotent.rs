//! PRD-mcphost-funnel-truth
//! AC5 (P1) — Given existing unlabeled rows, When `mcphost admin
//! backfill-origin` runs twice, Then the second run changes zero rows and
//! the printed counts match `select origin, count(*)`.

use crate::common;
use common::TempDataDir;
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::db::{Db, FunnelOriginTable};
use std::path::PathBuf;
use std::process::Command;

fn mcphost_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcphost"))
}

fn run_backfill(dir: &std::path::Path) -> std::process::Output {
    Command::new(mcphost_bin())
        .arg("admin")
        .arg("backfill-origin")
        .env("MCPHOST_DATA_DIR", dir)
        .output()
        .expect("run mcphost admin backfill-origin")
}

#[tokio::test]
async fn second_run_changes_nothing_and_printed_counts_match_select_counts() {
    let dir = TempDataDir::new();
    let db = Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");

    // A tenant that predates this PRD: its live classification already
    // marked it `source_class = fleet` (migration 0010, independent of
    // this PRD's own column), but `funnel_origin` was never touched --
    // exactly the "existing unlabeled row" this backfill exists for.
    let fleet_tenant = db
        .create_tenant_attributed(
            "Pre-PRD Fleet Tenant".to_string(),
            generate_namespace(),
            hash_key(&generate_key()),
            Some("harness:fleet-ip".to_string()),
            Some("fleet".to_string()),
            None,
            None,
            "synthetic".to_string(),
            Some("fleet".to_string()),
        )
        .await
        .expect("seed fleet tenant");
    assert_eq!(fleet_tenant.funnel_origin, "unknown");

    // A tenant with no fleet/probe signal at all -- the backfill must
    // leave it `unknown` rather than guessing (Goals: "stay queryable as
    // unknown where neither applies").
    let unclassified_tenant = db
        .create_tenant_attributed(
            "Pre-PRD Unclassified Tenant".to_string(),
            generate_namespace(),
            hash_key(&generate_key()),
            None,
            None,
            None,
            None,
            "external".to_string(),
            None,
        )
        .await
        .expect("seed unclassified tenant");
    assert_eq!(unclassified_tenant.funnel_origin, "unknown");

    // A claim-email journal row for the fleet tenant, inherited `unknown`
    // at insert time (same subselect `record_claim_email_failure` always
    // uses) since the owning tenant was still unknown when it was written.
    db.record_claim_email_failure(fleet_tenant.id, Some(500))
        .await
        .expect("seed claim email failure");

    let first = run_backfill(&dir.0);
    assert!(first.status.success(), "{first:?}");
    let first_stdout = String::from_utf8_lossy(&first.stdout).to_string();
    assert!(
        first_stdout.contains("tenants changed=1"),
        "only the fleet tenant should change on the first run: {first_stdout}"
    );
    assert!(
        first_stdout.contains("claim_email_events changed=1"),
        "the fleet tenant's own claim-email row should inherit the reclassification: {first_stdout}"
    );

    let reloaded_fleet = db
        .find_tenant_by_namespace(fleet_tenant.namespace.clone())
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(reloaded_fleet.funnel_origin, "fleet", "{reloaded_fleet:?}");
    let reloaded_unclassified = db
        .find_tenant_by_namespace(unclassified_tenant.namespace.clone())
        .await
        .expect("query")
        .expect("tenant exists");
    assert_eq!(
        reloaded_unclassified.funnel_origin, "unknown",
        "a tenant with no fleet/probe signal must stay unknown, not be guessed: {reloaded_unclassified:?}"
    );
    let claim_email_origin: String = {
        let counts = db.funnel_origin_counts(FunnelOriginTable::ClaimEmailEvents).await.expect("counts");
        counts
            .into_iter()
            .find(|(_, count)| *count == 1)
            .map(|(origin, _)| origin)
            .expect("exactly one claim_email_events row")
    };
    assert_eq!(claim_email_origin, "fleet");

    // Idempotency: the second run must change zero rows.
    let second = run_backfill(&dir.0);
    assert!(second.status.success(), "{second:?}");
    let second_stdout = String::from_utf8_lossy(&second.stdout).to_string();
    assert!(second_stdout.contains("tenants changed=0"), "{second_stdout}");
    assert!(second_stdout.contains("claim_email_events changed=0"), "{second_stdout}");

    // The printed per-table breakdown must match a direct `SELECT
    // funnel_origin, COUNT(*) ... GROUP BY funnel_origin`.
    for table in [FunnelOriginTable::Tenants, FunnelOriginTable::ClaimEmailEvents, FunnelOriginTable::OauthFunnelEvents]
    {
        let counts = db.funnel_origin_counts(table).await.expect("counts");
        for (origin, count) in counts {
            assert!(
                second_stdout.contains(&format!("{origin}={count}")),
                "printed report missing {origin}={count}: {second_stdout}"
            );
        }
    }
}
