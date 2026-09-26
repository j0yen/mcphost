//! loop/mcphost-fleet-ips requirement 3/4 -- `admin.reclassify_fleet_ips`'s
//! DB-level backfill: two `external` signups from a fleet IP (plus their
//! matching tenants/calls, joined by `created_unix` -- `signup_events`
//! carries no `tenant_id`, per the PRD's own note that this join was
//! verified exact on prod, 83/83 rows, 0 collisions) get reclassified to
//! `synthetic`/`fleet`/`harness:fleet-ip`; one `external` signup from a
//! non-fleet IP is untouched. Same "seed via the real API, then raw-UPDATE
//! only the columns the fixture needs pinned" pattern as
//! `provaudit_ac03_migration_backfill_counts.rs`'s own backfill test.

use crate::common;
use common::TempDataDir;
use mcphost::state::parse_fleet_ips;
use rusqlite::params;

#[tokio::test]
async fn reclassify_fleet_ips_backfills_matched_rows_only_and_is_idempotent() {
    let dir = TempDataDir::new();
    let db = mcphost::db::Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let db_path = dir.0.join("mcphost.db");

    // Three tenants, all seeded `external` (pre-PRD state) via the real
    // attributed-create path.
    let mut tenant_ids = Vec::new();
    for display_name in ["Fleet Box One", "Fleet Box Two", "Real External Caller"] {
        let key = mcphost::auth::generate_key();
        let namespace = mcphost::auth::generate_namespace();
        let tenant = db
            .create_tenant_attributed(
                display_name.to_string(),
                namespace,
                mcphost::auth::hash_key(&key),
                None,
                Some("external".to_string()),
                None,
                None,
                "external".to_string(),
                Some("external-unverified".to_string()),
            )
            .await
            .expect("seed tenant");
        tenant_ids.push(tenant.id);
    }

    // Three signup_events, seeded via the real path then pinned to exact
    // `created_unix`/`origin` values with a raw connection -- the same
    // "seed for real, then raw-UPDATE only what must be exact" split the
    // AC3 provenance backfill test uses for its `calls` rows.
    for source_ip in ["46.225.110.44", "178.105.64.66", "8.8.8.8"] {
        db.record_signup_event_attributed(source_ip.to_string(), None, None)
            .await
            .expect("seed signup event");
    }

    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");
    // Row 1: fleet IP #1, matched -- tenant[0]'s created_unix pinned equal.
    conn.execute(
        "UPDATE signup_events SET origin = 'external', origin_detail = 'external-unverified', \
         created_unix = 5001 WHERE source_ip = '46.225.110.44'",
        [],
    )
    .expect("pin signup 1");
    conn.execute(
        "UPDATE tenants SET created_unix = 5001 WHERE id = ?1",
        params![tenant_ids[0]],
    )
    .expect("pin tenant 1");
    // Row 2: fleet IP #2 (CIDR match, 10.0.0.0/8 below covers neither of
    // these -- both are exact-address entries), matched.
    conn.execute(
        "UPDATE signup_events SET origin = 'external', origin_detail = 'external-unverified', \
         created_unix = 5002 WHERE source_ip = '178.105.64.66'",
        [],
    )
    .expect("pin signup 2");
    conn.execute(
        "UPDATE tenants SET created_unix = 5002 WHERE id = ?1",
        params![tenant_ids[1]],
    )
    .expect("pin tenant 2");
    // Row 3: non-fleet IP, must remain untouched.
    conn.execute(
        "UPDATE signup_events SET origin = 'external', origin_detail = 'external-unverified', \
         created_unix = 5003 WHERE source_ip = '8.8.8.8'",
        [],
    )
    .expect("pin signup 3");
    conn.execute(
        "UPDATE tenants SET created_unix = 5003 WHERE id = ?1",
        params![tenant_ids[2]],
    )
    .expect("pin tenant 3");

    // One `calls` row per tenant, all seeded `external` -- same raw-INSERT
    // convention the AC3 provenance test uses (no `Db` method writes a
    // bare `calls` row without a real tool call happening).
    for tenant_id in &tenant_ids {
        conn.execute(
            "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, origin, origin_detail) \
             VALUES (?1, 'seed_tool', 'unix:0.0', 0, 1, 1, 'external', 'external-unverified')",
            params![tenant_id],
        )
        .expect("seed call row");
    }
    drop(conn);

    let fleet_ips = parse_fleet_ips(Some("46.225.110.44,178.105.64.66"));
    let counts = db.reclassify_fleet_ips(fleet_ips.clone()).await.expect("reclassify");
    assert_eq!(counts.signup_events, 2, "{counts:?}");
    assert_eq!(counts.tenants, 2, "{counts:?}");
    assert_eq!(counts.calls, 2, "{counts:?}");

    let conn = rusqlite::Connection::open(&db_path).expect("open raw db");

    // The two fleet-ip signups flipped; the third didn't.
    let (o1, od1, s1): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT origin, origin_detail, synthetic FROM signup_events WHERE source_ip = '46.225.110.44'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("read signup 1");
    assert_eq!((o1.as_str(), od1.as_deref(), s1.as_deref()), ("synthetic", Some("fleet"), Some("harness:fleet-ip")));

    let (o2, od2, s2): (String, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT origin, origin_detail, synthetic FROM signup_events WHERE source_ip = '178.105.64.66'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("read signup 2");
    assert_eq!((o2.as_str(), od2.as_deref(), s2.as_deref()), ("synthetic", Some("fleet"), Some("harness:fleet-ip")));

    let o3: String = conn
        .query_row(
            "SELECT origin FROM signup_events WHERE source_ip = '8.8.8.8'",
            [],
            |r| r.get(0),
        )
        .expect("read signup 3");
    assert_eq!(o3, "external", "non-fleet-ip signup must stay external");

    // The two matching tenants flipped; the third didn't.
    for tenant_id in [tenant_ids[0], tenant_ids[1]] {
        let (source_class, synthetic, origin, origin_detail, classified_by): (
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT source_class, synthetic, origin, origin_detail, classified_by FROM tenants WHERE id = ?1",
                params![tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .expect("read matched tenant");
        assert_eq!(source_class.as_deref(), Some("fleet"));
        assert_eq!(synthetic.as_deref(), Some("harness:fleet-ip"));
        assert_eq!(origin, "synthetic");
        assert_eq!(origin_detail.as_deref(), Some("fleet"));
        assert_eq!(classified_by.as_deref(), Some("reclassify-fleet-ips"));
    }
    let (origin3, source_class3): (String, Option<String>) = conn
        .query_row(
            "SELECT origin, source_class FROM tenants WHERE id = ?1",
            params![tenant_ids[2]],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("read untouched tenant");
    assert_eq!(origin3, "external", "non-fleet-ip tenant must stay external");
    assert_eq!(source_class3.as_deref(), Some("external"));

    // Calls follow their tenant: two flipped, one untouched.
    for tenant_id in [tenant_ids[0], tenant_ids[1]] {
        let (origin, origin_detail): (String, Option<String>) = conn
            .query_row(
                "SELECT origin, origin_detail FROM calls WHERE tenant_id = ?1",
                params![tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("read matched call");
        assert_eq!(origin, "synthetic");
        assert_eq!(origin_detail.as_deref(), Some("fleet"));
    }
    let call_origin3: String = conn
        .query_row(
            "SELECT origin FROM calls WHERE tenant_id = ?1",
            params![tenant_ids[2]],
            |r| r.get(0),
        )
        .expect("read untouched call");
    assert_eq!(call_origin3, "external");
    drop(conn);

    // Idempotent: running it again with nothing left `external` from a
    // fleet IP is a no-op.
    let counts_again = db.reclassify_fleet_ips(fleet_ips).await.expect("re-run reclassify");
    assert_eq!(counts_again, mcphost::db::ReclassifyFleetIpsCounts::default());
}

/// requirement 1: an empty/unset fleet list never reclassifies anything,
/// even a signup whose IP happens to be one an operator will later add.
#[tokio::test]
async fn reclassify_fleet_ips_is_a_no_op_with_no_fleet_ips_configured() {
    let dir = TempDataDir::new();
    let db = mcphost::db::Db::open(&dir.0).expect("open db");
    db.migrate().await.expect("migrate");

    db.record_signup_event_attributed("46.225.110.44".to_string(), None, None)
        .await
        .expect("seed signup event");
    {
        let conn = rusqlite::Connection::open(dir.0.join("mcphost.db")).expect("open raw db");
        conn.execute("UPDATE signup_events SET origin = 'external'", [])
            .expect("pin origin");
    }

    let counts = db
        .reclassify_fleet_ips(parse_fleet_ips(None))
        .await
        .expect("reclassify with empty fleet list");
    assert_eq!(counts, mcphost::db::ReclassifyFleetIpsCounts::default());
}
