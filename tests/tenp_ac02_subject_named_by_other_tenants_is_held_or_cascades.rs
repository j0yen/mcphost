//! PRD-mcphost-synthetic-tenant-purge
//! AC2 (P0) — Given a subject with a row in every table naming tenants by key
//! or name (enumerated at test time; two-sided set = § Cascade) and, per
//! two-sided column, a synthetic and a claimed two-marker tenant's row naming
//! it, When sweeps run, Then it is held, held_entangled names each
//! claimed-owned column, no row of either tenant changes; claimed rows gone, it
//! is purged, synthetic rows get their § Cascade outcome and no row names it.
//!
//! The second test derives the two-sided set from the migrated schema alone
//! (R3 (a) two tenant columns, (b) self-references, (c) the structural set over
//! `ON DELETE CASCADE` edges) and fails by table name when a table is added
//! without being classified in `src/tenant_refs.rs`.

use crate::common::TestServer;
use crate::common::tenp::{self, DAY, Kind};
use mcphost::tenant_refs::{self, TWO_SIDED};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// What every sweep rewrites on its own: its `prune_log` row, `blobs` rows whose file is absent
/// (marked missing) and `call_results` past `expires_unix`.
const SKIP: &[&str] = &["prune_log", "blobs", "call_results"];

type Planted = Vec<(String, i64)>;

/// One row of `owner` (the other tenant) naming `subject` through every
/// two-sided column in `TWO_SIDED`. Returns the rows planted.
fn plant_two_sided(conn: &Connection, subject: i64, owner: i64) -> Planted {
    let (s, o) = (subject.to_string(), owner.to_string());
    let mut rows: Planted = Vec::new();
    let mut put = |table: &str, set: &[(&str, &str)]| -> i64 {
        let set: Vec<(&str, String)> = set.iter().map(|(a, b)| (*a, b.to_string())).collect();
        let id = tenp::plant(conn, table, &set);
        rows.push((table.to_string(), id));
        id
    };
    put("calls", &[("tenant_id", &o), ("caller_tenant_id", &s)]);
    put("runs", &[("tenant_id", &o), ("caller_tenant_id", &s)]);
    put("usage_daily", &[("tenant_id", &o), ("caller_tenant_id", &s)]);
    put("blocks", &[("tenant_id", &o), ("blocked_tenant_id", &s)]);
    put("contacts", &[("tenant_id", &o), ("contact_tenant_id", &s)]);
    put("muted", &[("tenant_id", &o), ("muted_tenant_id", &s)]);
    put("tool_version_watermarks", &[("tenant_id", &o), ("caller_tenant_id", &s)]);
    put("tool_caller_limits", &[("tenant_id", &o), ("caller_tenant_id", &s)]);
    put("contact_requests", &[("from_tenant_id", &s), ("to_tenant_id", &o)]);
    put("contact_requests", &[("from_tenant_id", &o), ("to_tenant_id", &s)]);
    put("tasks", &[("poster_tenant_id", &s), ("recipient_tenant_id", &o)]);
    put("tasks", &[("poster_tenant_id", &o), ("recipient_tenant_id", &s)]);
    // The pair join table: the subject as inviter, then as invitee.
    put("invite_joins", &[("invite_id", "0"), ("inviter_tenant_id", &s), ("invitee_tenant_id", &o)]);
    put("invite_joins", &[("invite_id", "0"), ("inviter_tenant_id", &o), ("invitee_tenant_id", &s)]);
    // The structural set: the subject owns the parent, the other side owns the child's tenant column.
    let group = put("groups", &[("owner_tenant_id", &s)]);
    put("group_members", &[("group_id", &group.to_string()), ("member_tenant_id", &o)]);
    let channel_rowid = put("channels", &[("group_id", &group.to_string()), ("created_by", &o)]);
    // `channels.id` is text: the child rows name it, not the rowid.
    let channel: String = conn
        .query_row("SELECT id FROM channels WHERE rowid = ?1", [channel_rowid], |r| r.get(0))
        .expect("channel id");
    let channel = format!("'{channel}'");
    put("channel_posts", &[("channel_id", &channel), ("from_tenant_id", &o)]);
    put("channel_cursors", &[("channel_id", &channel), ("tenant_id", &o)]);
    let org = put("orgs", &[("owner_tenant_id", &s)]);
    put("org_members", &[("org_id", &org.to_string()), ("tenant_id", &o)]);
    let org_invite = put("invites", &[("org_id", &org.to_string()), ("inviter_tenant_id", &o)]);
    put("invite_joins", &[("invite_id", &org_invite.to_string()), ("inviter_tenant_id", &o), ("invitee_tenant_id", &o)]);
    let own_invite = put("invites", &[("inviter_tenant_id", &s)]);
    put("invite_joins", &[("invite_id", &own_invite.to_string()), ("inviter_tenant_id", &s), ("invitee_tenant_id", &o)]);
    conn.execute("UPDATE tenants SET invited_by_tenant_id = ?1, scratch_of = ?1 WHERE id = ?2", [subject, owner])
        .expect("self references");
    rows
}

/// Removes the rows `plant_two_sided` planted for `owner` and clears its self references.
fn remove_planted(conn: &Connection, planted: &Planted, owner: i64) {
    for (table, rowid) in planted {
        conn.execute(&format!("DELETE FROM \"{table}\" WHERE rowid = ?1"), [rowid]).expect("remove planted");
    }
    conn.execute("UPDATE tenants SET invited_by_tenant_id = NULL, scratch_of = NULL WHERE id = ?1", [owner])
        .expect("clear self references");
}

/// Rows of the six text columns that carry a namespace rather than an id.
fn plant_text_rows(conn: &Connection, namespace: &str, owner: i64) -> Vec<(String, i64, String, String)> {
    let o = owner.to_string();
    let q = |s: &str| format!("'{s}'");
    let mut out = Vec::new();
    let mut put = |table: &str, column: &str, value: String, extra: &[(&str, String)]| {
        let mut set: Vec<(&str, String)> = extra.to_vec();
        set.push((column, q(&value)));
        let id = tenp::plant(conn, table, &set);
        out.push((table.to_string(), id, column.to_string(), value));
    };
    put("runs", "tool_name", format!("{namespace}.shared"), &[("tenant_id", o.clone())]);
    put("tools", "forked_from", format!("{namespace}.tool"), &[("tenant_id", o.clone())]);
    put("messages", "from_address", namespace.to_string(), &[("from_tenant_id", o.clone())]);
    put("channel_posts", "from_address", namespace.to_string(), &[("from_tenant_id", o.clone())]);
    put("messages", "refused_json", format!("{{\"to\":\"{namespace}\"}}"), &[("from_tenant_id", o)]);
    out
}

/// A row in each table with a `tenant_id` and no foreign key, and the two same-tenant tables.
fn plant_no_fk_rows(conn: &Connection, subject: i64) {
    let s = subject.to_string();
    for table in tenant_refs::NO_FK_TENANT_TABLES.iter().filter(|t| **t != "doc_chunks_fts") {
        tenp::plant(conn, table, &[("tenant_id", s.clone())]);
    }
    conn.execute(
        "INSERT INTO doc_chunks_fts (text, tenant_id, document_id, chunk_no) VALUES ('words', ?1, 'd', 0)",
        [subject],
    )
    .expect("fts row");
    for table in ["notify_receipts", "oauth_jti_denylist"] {
        tenp::plant(conn, table, &[("tenant_id", s.clone())]);
    }
    for (table, column) in tenant_refs::SET_NULL {
        tenp::plant(conn, table, &[(column, s.clone())]);
    }
}

fn count_where(conn: &Connection, table: &str, column: &str, id: i64) -> i64 {
    tenp::count(conn, &format!("SELECT COUNT(*) FROM \"{table}\" WHERE \"{column}\" = {id}"))
}

fn exists(conn: &Connection, table: &str, rowid: i64) -> bool {
    tenp::count(conn, &format!("SELECT COUNT(*) FROM \"{table}\" WHERE rowid = {rowid}")) == 1
}

fn block(report: &mcphost::db::PruneReport) -> &Value {
    &report.synthetic
}

#[tokio::test]
async fn tenp_ac02_a_subject_named_by_a_real_users_row_is_held_and_a_synthetic_one_cascades() {
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.pragma_update(None, "foreign_keys", "OFF").expect("foreign_keys off");
    conn.busy_timeout(std::time::Duration::from_secs(30)).expect("busy timeout");
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 30 * DAY);
    tenp::widen_row_retention(&conn);

    // Four subjects, each named from every two-sided column by tenants of one other class.
    let subject = |ns: &str| {
        tenp::insert_subjects(&conn, ns, 1, now - 60 * DAY, now - 20 * DAY, "self_offboard");
        conn.query_row("SELECT id FROM tenants WHERE namespace = ?1", [format!("{ns}1")], |r| r.get::<_, i64>(0)).unwrap()
    };
    let s_both = subject("tenp-sub-both-");
    let s_label = subject("tenp-sub-label-");
    let s_origin = subject("tenp-sub-origin-");
    let s_external = subject("tenp-sub-external-");
    let ns_both = "tenp-sub-both-1";

    let t_syn = tenp::insert_tenant(&conn, "tenp-cp-syn", Kind::SyntheticImplicit, now);
    let t_explicit = tenp::insert_tenant(&conn, "tenp-cp-explicit", Kind::SyntheticExplicit, now);
    let t_claimed = tenp::insert_tenant(&conn, "tenp-cp-claimed", Kind::Claimed, now);
    let t_label = tenp::insert_tenant(&conn, "tenp-cp-label", Kind::LabelOnly, now);
    let t_origin = tenp::insert_tenant(&conn, "tenp-cp-origin", Kind::OriginOnly, now);
    let t_external = tenp::insert_tenant(&conn, "tenp-cp-external", Kind::External, now);

    let synthetic_rows = plant_two_sided(&conn, s_both, t_syn);
    let claimed_rows = plant_two_sided(&conn, s_both, t_claimed);
    plant_two_sided(&conn, s_label, t_label);
    plant_two_sided(&conn, s_origin, t_origin);
    plant_two_sided(&conn, s_external, t_external);
    let explicit_row = tenp::plant(
        &conn,
        "contacts",
        &[("tenant_id", t_explicit.to_string()), ("contact_tenant_id", s_both.to_string())],
    );
    let text_rows = plant_text_rows(&conn, ns_both, t_syn);
    for s in [s_both, s_label, s_origin, s_external] {
        plant_no_fk_rows(&conn, s);
    }
    let empty = tenp::fill_children(&conn, "tenp-sub-%", &[]);
    assert!(empty.is_empty(), "tables that took no fixture row: {empty:?}");
    tenp::seed_pre_prd_prune_log(&conn, now - 5 * DAY);

    // Sweep 1 carries the block for the first time: it reports and deletes nothing.
    let before = tenp::dump(&conn, SKIP);
    let first = server.state.db.prune_once().await.expect("first sweep");
    let report = block(&first);
    assert_eq!(report["purged"], 0, "{report}");
    assert_eq!(report["would_purge"], 0, "every subject is entangled: {report}");
    tenp::assert_same_dump(&before, &tenp::dump(&conn, SKIP), "the report-only sweep changed no row");

    // Sweep 2 deletes: still nothing, every subject is held, and nothing changes.
    let second = server.state.db.prune_once().await.expect("second sweep");
    let report = block(&second);
    assert_eq!(report["purged"], 0, "{report}");
    for r in TWO_SIDED {
        assert_eq!(report["held_entangled"][r.key], 4, "held_entangled names {}: {report}", r.key);
        assert_eq!(report["cross_refs"][r.key]["outcome"], r.outcome.as_str(), "{}: {report}", r.key);
        assert!(report["cross_refs"][r.key]["other"].as_i64().unwrap() >= 4, "{}: {report}", r.key);
    }
    for (table, column) in tenant_refs::SET_NULL {
        let entry = &report["cross_refs"][format!("{table}.{column}")];
        assert_eq!(entry["outcome"], "set_null", "{table}.{column}: {report}");
        assert!(entry["other"].is_null() && entry["rows"].as_i64().unwrap() >= 4, "{table}.{column}: {report}");
        assert!(report["held_entangled"].get(format!("{table}.{column}")).is_none(), "an author column never holds");
    }
    tenp::assert_same_dump(&before, &tenp::dump(&conn, SKIP), "a held subject and its neighbours' rows are untouched");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM admin_events WHERE action = 'tenant_purge_synthetic'"), 0);

    // The claimed tenant's rows go: the subject is no longer named by a real user's row.
    remove_planted(&conn, &claimed_rows, t_claimed);
    let third = server.state.db.prune_once().await.expect("third sweep");
    let report = block(&third);
    assert_eq!(report["purged"], 1, "{report}");
    for r in TWO_SIDED {
        assert_eq!(report["held_entangled"][r.key], 3, "the other three stay held on {}: {report}", r.key);
    }
    assert_eq!(report["cross_refs"]["contacts.contact_tenant_id"]["synthetic_explicit"], 1, "{report}");

    // The synthetic tenant's rows got their outcome; nothing names the subject.
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM tenants WHERE namespace = 'tenp-sub-both-1'"), 0);
    for (table, rowid) in &synthetic_rows {
        let survives = matches!(table.as_str(), "calls" | "runs");
        assert_eq!(exists(&conn, table, *rowid), survives, "{table} row {rowid} of the synthetic tenant");
    }
    assert_eq!(count_where(&conn, "calls", "caller_tenant_id", s_both), 0);
    assert_eq!(count_where(&conn, "runs", "caller_tenant_id", s_both), 0);
    assert!(!exists(&conn, "contacts", explicit_row), "the explicit synthetic tenant's row cascaded");
    let (invited, scratch): (Option<i64>, Option<i64>) = conn
        .query_row("SELECT invited_by_tenant_id, scratch_of FROM tenants WHERE id = ?1", [t_syn], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!((invited, scratch), (None, None), "the self references are NULLed");
    assert_eq!(tenp::count(&conn, &format!("SELECT COUNT(*) FROM tenants WHERE id = {t_syn}")), 1, "the other tenant survives");
    for (table, column) in tenant_columns_excluding_tenants(&conn) {
        assert_eq!(count_where(&conn, &table, &column, s_both), 0, "{table}.{column} still names the purged subject");
    }
    assert_eq!(tenp::count(&conn, &format!("SELECT COUNT(*) FROM doc_chunks_fts WHERE tenant_id = {s_both}")), 0);
    for (table, column) in tenant_refs::SET_NULL {
        assert!(tenp::count(&conn, &format!("SELECT COUNT(*) FROM \"{table}\"")) >= 1, "{table} rows survive");
        assert_eq!(count_where(&conn, table, column, s_both), 0, "{table}.{column} set to NULL");
    }
    // Text that carries the namespace is never rewritten.
    for (table, rowid, column, value) in &text_rows {
        let now_value: String = conn
            .query_row(&format!("SELECT \"{column}\" FROM \"{table}\" WHERE rowid = ?1"), [rowid], |r| r.get(0))
            .unwrap();
        assert_eq!(&now_value, value, "{table}.{column} is left alone");
    }
    // The audit row carries the namespace and the subject's counts.
    let detail: String = conn
        .query_row(
            "SELECT detail FROM admin_events WHERE action = 'tenant_purge_synthetic' AND tenant = ?1",
            [ns_both],
            |r| r.get(0),
        )
        .expect("one audit row for the subject");
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["cross_refs"]["contacts.contact_tenant_id"]["outcome"], "cascade", "{detail}");
    // The held subjects are still there with their claimed neighbours' rows.
    for ns in ["tenp-sub-label-1", "tenp-sub-origin-1", "tenp-sub-external-1"] {
        assert_eq!(tenp::count(&conn, &format!("SELECT COUNT(*) FROM tenants WHERE namespace = '{ns}'")), 1, "{ns} is held");
    }
    assert!(claimed_rows.iter().all(|(table, rowid)| !exists(&conn, table, *rowid)));
    let _ = (t_label, t_origin, t_external);
}

/// Tenant-id columns of every table but `tenants`, plus the two self references on `tenants`.
fn tenant_columns_excluding_tenants(conn: &Connection) -> Vec<(String, String)> {
    tenp::tenant_columns(conn)
        .into_iter()
        .filter(|(table, column)| table != "tenants" || matches!(column.as_str(), "invited_by_tenant_id" | "scratch_of"))
        .collect()
}

/// Tables with a tenant column, as the schema alone defines it.
fn derive_from_schema(conn: &Connection) -> (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>) {
    let by_table: BTreeMap<String, BTreeSet<String>> = tenp::tenant_columns(conn)
        .into_iter()
        .fold(BTreeMap::new(), |mut m, (t, c)| {
            m.entry(t).or_default().insert(c);
            m
        });
    // (a) a table with two tenant columns, (b) a self-reference on `tenants`.
    let flat: BTreeSet<String> = by_table
        .iter()
        .filter(|(t, cols)| (t.as_str() != "tenants" && cols.len() >= 2) || (t.as_str() == "tenants" && cols.iter().any(|c| c != "id")))
        .map(|(t, _)| t.clone())
        .collect();
    // (c) a table with a tenant column that reaches, over ON DELETE CASCADE foreign keys child -> parent,
    // a parent whose own tenant column is a CASCADE foreign key to tenants.
    let cascade_tenant: BTreeSet<String> = by_table
        .keys()
        .filter(|t| {
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_list(?1) WHERE \"table\" = 'tenants' AND on_delete = 'CASCADE'",
                [t],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
                > 0
        })
        .cloned()
        .collect();
    let edges = |table: &str| -> Vec<String> {
        conn.prepare("SELECT \"table\" FROM pragma_foreign_key_list(?1) WHERE \"table\" <> 'tenants' AND on_delete = 'CASCADE'")
            .unwrap()
            .query_map([table], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let (mut members, mut parents) = (BTreeSet::new(), BTreeSet::new());
    for table in by_table.keys().filter(|t| t.as_str() != "tenants") {
        let mut stack = edges(table);
        let mut seen = BTreeSet::new();
        while let Some(parent) = stack.pop() {
            if !seen.insert(parent.clone()) {
                continue;
            }
            if cascade_tenant.contains(&parent) {
                members.insert(table.clone());
                parents.insert(parent.clone());
            }
            stack.extend(edges(&parent));
        }
    }
    (flat, members, parents)
}

#[tokio::test]
async fn tenp_ac02_two_sided_set_equals_the_schema_derived_set() {
    let server = TestServer::start().await;
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let (flat, members, parents) = derive_from_schema(&conn);

    let registry_flat: BTreeSet<String> = TWO_SIDED
        .iter()
        .filter(|r| !r.key.contains('/'))
        .map(|r| r.key.split_once('.').unwrap().0.to_string())
        .collect();
    assert_eq!(flat, registry_flat, "tables with two tenant columns or a self reference: schema vs src/tenant_refs.rs");

    let join_children: BTreeSet<String> = TWO_SIDED
        .iter()
        .filter_map(|r| r.key.split_once('/'))
        .map(|(_, child)| child.split_once('.').unwrap().0.to_string())
        .collect();
    let registry_members: BTreeSet<String> =
        join_children.iter().cloned().chain(tenant_refs::SAME_TENANT.iter().map(|t| t.to_string())).collect();
    assert_eq!(members, registry_members, "the structural set: schema vs src/tenant_refs.rs (classify a new member)");

    let join_parents: BTreeSet<String> = TWO_SIDED
        .iter()
        .filter_map(|r| r.key.split_once('/'))
        .map(|(parent, _)| parent.split_once('.').unwrap().0.to_string())
        .collect();
    assert!(join_parents.is_subset(&parents), "{join_parents:?} are parents the closure reaches: {parents:?}");
    let named = tenant_refs::two_sided_tables();
    assert!(flat.iter().all(|t| named.contains(t.as_str())), "every flat table is named by a key");

    // Every foreign-key column to tenants that is not cascade-or-classified: a SET NULL author
    // column must be in SET_NULL, so a new author column fails here by name.
    let set_null: BTreeSet<(String, String)> = conn
        .prepare(
            "SELECT m.name, f.\"from\" FROM sqlite_master m, pragma_foreign_key_list(m.name) f \
             WHERE m.type = 'table' AND f.\"table\" = 'tenants' AND f.on_delete = 'SET NULL'",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let registry_set_null: BTreeSet<(String, String)> =
        tenant_refs::SET_NULL.iter().map(|(t, c)| (t.to_string(), c.to_string())).collect();
    assert_eq!(set_null, registry_set_null, "SET NULL author columns: schema vs src/tenant_refs.rs");

    // The NO ACTION columns are the ones the delete must NULL first.
    let no_action: BTreeSet<(String, String)> = conn
        .prepare(
            "SELECT m.name, f.\"from\" FROM sqlite_master m, pragma_foreign_key_list(m.name) f \
             WHERE m.type = 'table' AND f.\"table\" = 'tenants' AND f.on_delete = 'NO ACTION'",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let nulled: BTreeSet<(String, String)> = tenant_refs::NULLED.iter().map(|(t, c)| (t.to_string(), c.to_string())).collect();
    assert!(no_action.is_subset(&nulled), "a NO ACTION column to tenants fails the delete unless NULLed: {no_action:?}");
}
