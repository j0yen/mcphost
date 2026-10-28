//! PRD-mcphost-synthetic-tenant-purge
//! AC2 (P0) — Given a subject with a row in every table naming tenants by key or name (enumerated at
//! test time; two-sided set = § Cascade) and, per two-sided column, a synthetic and a claimed
//! two-marker tenant's row naming it, When sweeps run, Then it is held, held_entangled names each
//! claimed-owned column, no row of either tenant changes; claimed rows gone, it is purged, synthetic
//! rows get their § Cascade outcome and no row names it.

use crate::common::tenp::{self, DAY, Planter, Row};
use crate::common::{EnvGuard, TestServer};
use mcphost::db::{CROSS_REFS, PurgeOutcome};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// § Cascade, cross-tenant: tables whose rows name a subject from another tenant's row, or cascade
/// from another tenant's parent row. A table added later that fits (a), (b) or (c) below fails
/// `derived_two_sided_tables_equal_the_cascade_classification` by name until it is listed here
/// (and in `db::CROSS_REFS`).
const CROSS_TENANT: &[&str] = &[
    "blocks", "calls", "channel_cursors", "channel_posts", "channels", "contact_requests", "contacts",
    "group_members", "invite_joins", "invites", "muted", "org_members", "runs", "tasks", "tenants",
    "tool_caller_limits", "tool_version_watermarks", "usage_daily",
];
/// § Cascade, same-tenant: cascade from a parent that always holds the child's own tenant.
const SAME_TENANT: &[&str] = &["notify_receipts", "oauth_jti_denylist"];
/// Tenant-id columns with no foreign key that the cascade clears by statement (§ Cascade).
const NAME_ONLY: &[(&str, &str)] = &[
    ("call_traces", "tenant_id"),
    ("doc_chunks_fts", "tenant_id"),
    ("help_search_events", "tenant_id"),
    ("index_chunks", "tenant_id"),
    ("index_sources", "tenant_id"),
    ("invite_joins", "inviter_tenant_id"),
    ("network_denials", "tenant_id"),
    ("oauth_issuers", "tenant_id"),
    ("runs", "caller_tenant_id"),
    ("tenants", "scratch_of"),
    ("usage_daily", "caller_tenant_id"),
];
/// Named like a tenant column but not one: an admin label.
const NOT_A_TENANT: &[(&str, &str)] = &[("bans", "created_by")];

fn is_tenant_name(col: &str) -> bool {
    col == "tenant_id" || col.ends_with("_tenant_id") || col == "created_by" || col == "scratch_of"
}

fn tables(conn: &Connection) -> Vec<String> {
    conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    conn.prepare("SELECT name FROM pragma_table_info(?1)")
        .unwrap()
        .query_map(params![table], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Every column that names a tenant, by foreign key or by name: `(table, column, is_fk)`.
fn tenant_columns(conn: &Connection) -> Vec<(String, String, bool)> {
    let fks: BTreeSet<(String, String)> =
        tenp::tenant_fk_cols(conn).into_iter().map(|f| (f.table, f.column)).collect();
    let mut out = Vec::new();
    for t in tables(conn) {
        for c in columns(conn, &t) {
            let is_fk = fks.contains(&(t.clone(), c.clone()));
            if (is_fk || is_tenant_name(&c)) && !NOT_A_TENANT.contains(&(t.as_str(), c.as_str())) {
                out.push((t.clone(), c, is_fk));
            }
        }
    }
    out
}

/// (a) ∪ (b) ∪ (c) of R3, derived from the migrated schema.
fn derived_two_sided(conn: &Connection) -> BTreeSet<String> {
    let cols = tenant_columns(conn);
    let mut per_table: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
    for (t, c, fk) in &cols {
        per_table.entry(t.clone()).or_default().push((c.clone(), *fk));
    }
    let mut set = BTreeSet::new();
    // (a) a table with two tenant columns, which covers (b), a self-reference on `tenants`.
    for (t, cs) in &per_table {
        if cs.len() >= 2 {
            set.insert(t.clone());
        }
    }
    // (c) a table whose foreign-key tenant column reaches, over ON DELETE CASCADE edges child ->
    // parent, a parent whose own tenant column is ON DELETE CASCADE.
    let fk_cascade: BTreeSet<String> =
        tenp::tenant_fk_cols(conn).into_iter().filter(|f| f.on_delete == "CASCADE").map(|f| f.table).collect();
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in tables(conn) {
        let mut stmt = conn
            .prepare("SELECT \"table\" FROM pragma_foreign_key_list(?1) WHERE on_delete = 'CASCADE' AND \"table\" <> 'tenants'")
            .unwrap();
        let parents: Vec<String> = stmt.query_map(params![t], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        edges.insert(t, parents);
    }
    fn reaches(edges: &BTreeMap<String, Vec<String>>, from: &str, goal: &BTreeSet<String>, depth: usize) -> bool {
        depth < 6
            && edges
                .get(from)
                .is_some_and(|ps| ps.iter().any(|p| goal.contains(p) || reaches(edges, p, goal, depth + 1)))
    }
    for (t, cs) in &per_table {
        if cs.iter().any(|(_, fk)| *fk) && reaches(&edges, t, &fk_cascade, 0) {
            set.insert(t.clone());
        }
    }
    set
}

#[tokio::test]
async fn tenp_ac02_derived_two_sided_tables_equal_the_cascade_classification() {
    let server = TestServer::start().await;
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let derived = derived_two_sided(&conn);
    let classified: BTreeSet<String> = CROSS_TENANT.iter().chain(SAME_TENANT).map(|t| t.to_string()).collect();
    let unclassified: Vec<_> = derived.difference(&classified).collect();
    let stale: Vec<_> = classified.difference(&derived).collect();
    assert!(unclassified.is_empty(), "tables that name two tenants or cascade from another tenant's row and are not classified in § Cascade: {unclassified:?}");
    assert!(stale.is_empty(), "classified tables the schema no longer derives: {stale:?}");

    // Every name-only tenant column (no foreign key) is the cascade's explicit statements' business.
    let name_only: BTreeSet<(String, String)> = tenant_columns(&conn)
        .into_iter()
        .filter(|(_, _, fk)| !fk)
        .map(|(t, c, _)| (t, c))
        .collect();
    let mut expected: BTreeSet<(String, String)> = NAME_ONLY.iter().map(|(t, c)| (t.to_string(), c.to_string())).collect();
    // The FTS5 table is virtual and lists no `foreign_key_list`; `pragma_table_info` still names it.
    expected.insert(("doc_chunks_fts".to_string(), "tenant_id".to_string()));
    assert_eq!(name_only, expected, "a tenant column without a foreign key must be cleared by delete_tenant_rows");

    // The implementation's reference table covers every classified cross-tenant table, and only those.
    let in_table: BTreeSet<String> = CROSS_REFS
        .iter()
        .filter(|r| r.outcome != PurgeOutcome::SetNull)
        .map(|r| r.key.rsplit('/').next().unwrap().split('.').next().unwrap().to_string())
        .collect();
    let cross: BTreeSet<String> = CROSS_TENANT.iter().map(|t| t.to_string()).collect();
    assert_eq!(in_table, cross, "db::CROSS_REFS and § Cascade agree on the cross-tenant tables");
}

/// How a cross reference is planted and found again.
struct Ref {
    key: &'static str,
    /// The row that names the subject, in the OTHER tenant's side of the relation.
    table: &'static str,
    /// The column holding the subject, where the relation has one.
    subject: Option<&'static str>,
    outcome: PurgeOutcome,
}

fn refs() -> Vec<Ref> {
    CROSS_REFS
        .iter()
        .filter(|r| r.outcome != PurgeOutcome::SetNull)
        .map(|r| {
            let (table, _owner, subject) = match r.key {
                "calls.caller_tenant_id" => ("calls", "tenant_id", Some("caller_tenant_id")),
                "runs.caller_tenant_id" => ("runs", "tenant_id", Some("caller_tenant_id")),
                "tenants.invited_by_tenant_id" => ("tenants", "id", Some("invited_by_tenant_id")),
                "tenants.scratch_of" => ("tenants", "id", Some("scratch_of")),
                "usage_daily.caller_tenant_id" => ("usage_daily", "tenant_id", Some("caller_tenant_id")),
                "invite_joins.inviter_tenant_id" => ("invite_joins", "invitee_tenant_id", Some("inviter_tenant_id")),
                "blocks.blocked_tenant_id" => ("blocks", "tenant_id", Some("blocked_tenant_id")),
                "contacts.contact_tenant_id" => ("contacts", "tenant_id", Some("contact_tenant_id")),
                "muted.muted_tenant_id" => ("muted", "tenant_id", Some("muted_tenant_id")),
                "tool_version_watermarks.caller_tenant_id" => ("tool_version_watermarks", "tenant_id", Some("caller_tenant_id")),
                "tool_caller_limits.caller_tenant_id" => ("tool_caller_limits", "tenant_id", Some("caller_tenant_id")),
                "contact_requests.from_tenant_id" => ("contact_requests", "to_tenant_id", Some("from_tenant_id")),
                "contact_requests.to_tenant_id" => ("contact_requests", "from_tenant_id", Some("to_tenant_id")),
                "tasks.poster_tenant_id" => ("tasks", "recipient_tenant_id", Some("poster_tenant_id")),
                "tasks.recipient_tenant_id" => ("tasks", "poster_tenant_id", Some("recipient_tenant_id")),
                "invite_joins.invitee_tenant_id" => ("invite_joins", "inviter_tenant_id", Some("invitee_tenant_id")),
                "groups.owner_tenant_id/group_members.member_tenant_id" => ("group_members", "member_tenant_id", None),
                "groups.owner_tenant_id/channels.created_by" => ("channels", "created_by", None),
                "groups.owner_tenant_id/channel_posts.from_tenant_id" => ("channel_posts", "from_tenant_id", None),
                "groups.owner_tenant_id/channel_cursors.tenant_id" => ("channel_cursors", "tenant_id", None),
                "orgs.owner_tenant_id/org_members.tenant_id" => ("org_members", "tenant_id", None),
                "orgs.owner_tenant_id/invites.inviter_tenant_id" => ("invites", "inviter_tenant_id", None),
                "orgs.owner_tenant_id/invite_joins.invitee_tenant_id" => ("invite_joins", "invitee_tenant_id", None),
                "invites.inviter_tenant_id/invite_joins.invitee_tenant_id" => ("invite_joins", "invitee_tenant_id", None),
                other => panic!("cross reference {other} has no plant recipe in tenp_ac02: add one"),
            };
            Ref { key: r.key, table, subject, outcome: r.outcome }
        })
        .collect()
}

fn int(v: i64) -> Sql {
    Sql::Integer(v)
}

fn text(v: &str) -> Sql {
    Sql::Text(v.to_string())
}

/// Plants the row of tenant `x` that names subject `s` through `key`; returns its rowid.
fn plant_ref(conn: &Connection, p: &Planter, key: &str, s: i64, x: i64, k: i64) -> i64 {
    let ok = |r: rusqlite::Result<()>| r.unwrap_or_else(|e| panic!("plant {key}: {e}"));
    let flat = |table: &str, owner: &str, subject: &str| {
        ok(p.plant_with(conn, table, owner, x, k, &[(subject, int(s))]));
        conn.last_insert_rowid()
    };
    match key {
        "calls.caller_tenant_id" => flat("calls", "tenant_id", "caller_tenant_id"),
        "runs.caller_tenant_id" => flat("runs", "tenant_id", "caller_tenant_id"),
        "usage_daily.caller_tenant_id" => flat("usage_daily", "tenant_id", "caller_tenant_id"),
        "blocks.blocked_tenant_id" => flat("blocks", "tenant_id", "blocked_tenant_id"),
        "contacts.contact_tenant_id" => flat("contacts", "tenant_id", "contact_tenant_id"),
        "muted.muted_tenant_id" => flat("muted", "tenant_id", "muted_tenant_id"),
        "tool_version_watermarks.caller_tenant_id" => flat("tool_version_watermarks", "tenant_id", "caller_tenant_id"),
        "tool_caller_limits.caller_tenant_id" => flat("tool_caller_limits", "tenant_id", "caller_tenant_id"),
        "invite_joins.inviter_tenant_id" => flat("invite_joins", "invitee_tenant_id", "inviter_tenant_id"),
        "tenants.invited_by_tenant_id" | "tenants.scratch_of" => {
            let col = key.split('.').nth(1).unwrap();
            conn.execute(&format!("UPDATE tenants SET {col} = ?1 WHERE id = ?2"), params![s, x]).unwrap();
            x
        }
        "contact_requests.from_tenant_id" => {
            ok(p.plant_with(conn, "contact_requests", "from_tenant_id", s, k, &[("to_tenant_id", int(x))]));
            conn.last_insert_rowid()
        }
        "contact_requests.to_tenant_id" => {
            ok(p.plant_with(conn, "contact_requests", "to_tenant_id", s, k, &[("from_tenant_id", int(x))]));
            conn.last_insert_rowid()
        }
        "tasks.poster_tenant_id" => {
            ok(p.plant_with(conn, "tasks", "poster_tenant_id", s, k, &[("recipient_tenant_id", int(x))]));
            conn.last_insert_rowid()
        }
        "tasks.recipient_tenant_id" => {
            ok(p.plant_with(conn, "tasks", "recipient_tenant_id", s, k, &[("poster_tenant_id", int(x))]));
            conn.last_insert_rowid()
        }
        "invite_joins.invitee_tenant_id" => {
            ok(p.plant_with(conn, "invite_joins", "invitee_tenant_id", s, k, &[("inviter_tenant_id", int(x))]));
            conn.last_insert_rowid()
        }
        "groups.owner_tenant_id/group_members.member_tenant_id" => {
            let g = group_of(conn, p, s, k);
            ok(p.plant_with(conn, "group_members", "member_tenant_id", x, k, &[("group_id", int(g))]));
            conn.last_insert_rowid()
        }
        "groups.owner_tenant_id/channels.created_by" => {
            let g = group_of(conn, p, s, k);
            ok(p.plant_with(conn, "channels", "created_by", x, k, &[("group_id", int(g))]));
            conn.last_insert_rowid()
        }
        "groups.owner_tenant_id/channel_posts.from_tenant_id" => {
            let c = group_channel_of(conn, p, s, k);
            ok(p.plant_with(conn, "channel_posts", "from_tenant_id", x, k, &[("channel_id", text(&c))]));
            conn.last_insert_rowid()
        }
        "groups.owner_tenant_id/channel_cursors.tenant_id" => {
            let c = group_channel_of(conn, p, s, k);
            ok(p.plant_with(conn, "channel_cursors", "tenant_id", x, k, &[("channel_id", text(&c))]));
            conn.last_insert_rowid()
        }
        "orgs.owner_tenant_id/org_members.tenant_id" => {
            let o = org_of(conn, p, s, k);
            ok(p.plant_with(conn, "org_members", "tenant_id", x, k, &[("org_id", int(o))]));
            conn.last_insert_rowid()
        }
        "orgs.owner_tenant_id/invites.inviter_tenant_id" => {
            let o = org_of(conn, p, s, k);
            ok(p.plant_with(conn, "invites", "inviter_tenant_id", x, k, &[("org_id", int(o))]));
            conn.last_insert_rowid()
        }
        "orgs.owner_tenant_id/invite_joins.invitee_tenant_id" => {
            let o = org_of(conn, p, s, k);
            ok(p.plant_with(conn, "invites", "inviter_tenant_id", s, k, &[("org_id", int(o))]));
            let invite = conn.last_insert_rowid();
            ok(p.plant_with(conn, "invite_joins", "invitee_tenant_id", x, k, &[("invite_id", int(invite)), ("inviter_tenant_id", int(s))]));
            conn.last_insert_rowid()
        }
        "invites.inviter_tenant_id/invite_joins.invitee_tenant_id" => {
            ok(p.plant_with(conn, "invites", "inviter_tenant_id", s, k, &[]));
            let invite = conn.last_insert_rowid();
            ok(p.plant_with(conn, "invite_joins", "invitee_tenant_id", x, k, &[("invite_id", int(invite)), ("inviter_tenant_id", int(s))]));
            conn.last_insert_rowid()
        }
        other => panic!("no plant recipe for {other}"),
    }
}

/// A group owned by `s` (one per call).
fn group_of(conn: &Connection, p: &Planter, s: i64, k: i64) -> i64 {
    p.plant(conn, "groups", "owner_tenant_id", s, 7_000 + k).expect("plant group");
    conn.last_insert_rowid()
}

/// A channel of a group owned by `s`; returns the channel's text id.
fn group_channel_of(conn: &Connection, p: &Planter, s: i64, k: i64) -> String {
    let g = group_of(conn, p, s, k);
    p.plant_with(conn, "channels", "created_by", s, 7_000 + k, &[("group_id", int(g))]).expect("plant channel");
    conn.query_row("SELECT id FROM channels WHERE group_id = ?1", params![g], |r| r.get(0)).unwrap()
}

/// An org owned by `s`.
fn org_of(conn: &Connection, p: &Planter, s: i64, k: i64) -> i64 {
    p.plant(conn, "orgs", "owner_tenant_id", s, 7_000 + k).expect("plant org");
    conn.last_insert_rowid()
}

/// `true` while the planted row exists; `Some(is_null)` of its subject column for a nulled outcome.
fn probe(conn: &Connection, r: &Ref, rowid: i64) -> Option<bool> {
    let table = r.table;
    let pk = if table == "tenants" { "id" } else { "rowid" };
    let exists: i64 =
        conn.query_row(&format!("SELECT COUNT(*) FROM \"{table}\" WHERE {pk} = ?1"), params![rowid], |x| x.get(0)).unwrap();
    if exists == 0 {
        return None;
    }
    Some(match r.subject {
        Some(col) => conn
            .query_row(&format!("SELECT {col} IS NULL FROM \"{table}\" WHERE {pk} = ?1"), params![rowid], |x| x.get::<_, i64>(0))
            .unwrap()
            != 0,
        None => false,
    })
}

/// Every row that names one of `ids` through a tenant column, rendered, per table: what must not
/// change while a subject is held. The sweep's own journals are excluded.
fn snapshot(conn: &Connection, ids: &[i64]) -> BTreeMap<String, Vec<String>> {
    let mut by_table: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (t, c, _) in tenant_columns(conn) {
        // The FTS table has no rowid view; the blob sweep flags a row whose file is absent (`missing`).
        if t == "doc_chunks_fts" || t == "blobs" {
            continue;
        }
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let mut stmt = conn.prepare(&format!("SELECT rowid, * FROM \"{t}\" WHERE \"{c}\" IN ({list})")).unwrap();
        let n = stmt.column_count();
        let rows: Vec<String> = stmt
            .query_map([], |r| {
                let mut s = String::new();
                for i in 0..n {
                    s.push_str(&format!("{:?}|", r.get::<_, Sql>(i)?));
                }
                Ok(s)
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        by_table.entry(t).or_default().extend(rows);
    }
    for rows in by_table.values_mut() {
        rows.sort();
        rows.dedup();
    }
    by_table.retain(|_, v| !v.is_empty());
    by_table
}

/// The tables whose rows differ between two snapshots, with the first differing row of each.
fn snapshot_diff(a: &BTreeMap<String, Vec<String>>, b: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = Vec::new();
    for t in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
        let (x, y) = (a.get(t).cloned().unwrap_or_default(), b.get(t).cloned().unwrap_or_default());
        if x != y {
            let gone = x.iter().find(|r| !y.contains(r)).cloned().unwrap_or_default();
            let new = y.iter().find(|r| !x.contains(r)).cloned().unwrap_or_default();
            out.push(format!("{t}: {} -> {} rows; first gone {gone}; first new {new}", x.len(), y.len()));
        }
    }
    out.join("\n")
}

/// Rows of every tenant column that still equal `id`.
fn rows_naming(conn: &Connection, id: i64) -> Vec<String> {
    let mut left = Vec::new();
    for (t, c, _) in tenant_columns(conn) {
        let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM \"{t}\" WHERE \"{c}\" = ?1"), params![id], |r| r.get(0)).unwrap();
        if n > 0 {
            left.push(format!("{t}.{c} x{n}"));
        }
    }
    left
}

fn owner_class(class: &str, ns: &str, now: i64) -> Row {
    let base = Row::implicit(ns, now, 5, 3600);
    match class {
        "synthetic_implicit" => base,
        "synthetic_explicit" => Row { signup_source: Some("explicit"), ..base },
        "label_only" => Row { origin: "external", ..base },
        "origin_only" => Row { synthetic: None, ..base },
        "neither" => Row { origin: "external", synthetic: None, ..base },
        "claimed" => Row { claimed_unix: Some(now - DAY), ..base },
        "paid" => Row { paid_unix: Some(now - DAY), ..base },
        "billing_ref" => Row { billing_ref: Some("cus_tenp"), ..base },
        "owner_verified" => Row { owner_verified_at: Some(now - DAY), ..base },
        other => panic!("unknown class {other}"),
    }
}

const OTHER_CLASSES: [&str; 7] =
    ["label_only", "origin_only", "neither", "claimed", "paid", "billing_ref", "owner_verified"];

#[tokio::test]
async fn tenp_ac02_a_subject_with_other_class_rows_is_held_then_purged_once_they_are_gone() {
    // One `EnvGuard` per test (one process-wide lock); the grace and idle envs stay unset.
    let _env = EnvGuard::clear(tenp::BUDGET_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 40 * DAY);
    let mut planter = Planter::new(&conn, now - 3600);
    for (t, c) in NAME_ONLY {
        if !planter.has(t, c) && *t != "tenants" && *t != "doc_chunks_fts" {
            planter.add_plain(&conn, t, c);
        }
    }
    planter.add_plain(&conn, "doc_chunks_fts", "tenant_id");

    let two_sided = refs();
    assert!(two_sided.len() >= 24, "the reference table lists every two-sided column: {}", two_sided.len());
    let keys: Vec<&str> = two_sided.iter().map(|r| r.key).collect();
    let own_skip: BTreeSet<String> = CROSS_REFS
        .iter()
        .filter(|r| r.outcome != PurgeOutcome::SetNull && !r.key.contains('/'))
        .map(|r| r.key.to_string())
        .collect();

    // Subject rows of its own: one per planted (table, column), except the columns where the subject
    // is the named side of another tenant's row (those are planted per owner below).
    let own_rows = |conn: &Connection, s: i64| {
        for (i, (table, column)) in planter.keys().into_iter().enumerate() {
            if own_skip.contains(&format!("{table}.{column}")) || table == "tenant_lifecycle_events" {
                continue;
            }
            planter.plant(conn, &table, &column, s, i as i64).unwrap_or_else(|e| panic!("plant {table}.{column}: {e}"));
        }
    };

    // The mixed subject: synthetic rows (implicit and explicit owners alternate) and a claimed
    // tenant's row for every two-sided column. `plain` has no reference at all.
    let mix = tenp::insert(&conn, &Row::purge_subject("tenp-mix", now, "idle_expired", 60, 30));
    let plain = tenp::insert(&conn, &Row::purge_subject("tenp-plain", now, "self_offboard", 60, 30));
    own_rows(&conn, mix);
    own_rows(&conn, plain);
    let owners_syn = [
        tenp::insert(&conn, &owner_class("synthetic_implicit", "tenp-owner-implicit", now)),
        tenp::insert(&conn, &owner_class("synthetic_explicit", "tenp-owner-explicit", now)),
    ];
    let claimed = tenp::insert(&conn, &owner_class("claimed", "tenp-owner-claimed", now));
    let mut planted_syn = Vec::new();
    let mut planted_claimed = Vec::new();
    for (i, r) in two_sided.iter().enumerate() {
        let k = i as i64;
        planted_syn.push(plant_ref(&conn, &planter, r.key, mix, owners_syn[i % 2], k));
        planted_claimed.push(plant_ref(&conn, &planter, r.key, mix, claimed, 100 + k));
    }
    // Text columns that carry the subject's namespace: another tenant's run and fork, the subject's
    // own message and post. A text column is never rewritten.
    let ns = "tenp-mix";
    let text_rows = plant_text_columns(&conn, &planter, mix, ns, owners_syn[0]);

    // One subject per other class, each with a row in every two-sided column.
    let mut class_subjects = Vec::new();
    for (n, class) in OTHER_CLASSES.iter().enumerate() {
        let s = tenp::insert(&conn, &Row::purge_subject(&format!("tenp-held-{class}"), now, "idle_expired", 60, 30));
        let x = tenp::insert(&conn, &owner_class(class, &format!("tenp-cowner-{class}"), now));
        for (i, r) in two_sided.iter().enumerate() {
            plant_ref(&conn, &planter, r.key, s, x, 200 + 100 * n as i64 + i as i64);
        }
        class_subjects.push((class.to_string(), s, x));
    }

    let involved: Vec<i64> = [mix, plain, claimed, owners_syn[0], owners_syn[1]]
        .into_iter()
        .chain(class_subjects.iter().flat_map(|(_, s, x)| [*s, *x]))
        .collect();
    let before = snapshot(&conn, &involved);
    let held_snapshot_ids: Vec<i64> = involved.iter().copied().filter(|i| *i != plain).collect();
    let held_before = snapshot(&conn, &held_snapshot_ids);

    // Sweep 1 is the report-only one: it names every held column and deletes nothing.
    server.state.db.prune_once().await.expect("first sweep");
    let first = tenp::last_block(&conn);
    assert_eq!(first["purged"], 0, "{first}");
    assert!(tenp::exists(&conn, plain) && tenp::exists(&conn, mix));
    let subjects_entangled = 1 + OTHER_CLASSES.len() as i64;
    for r in &two_sided {
        // `>=`: a parent planted for one relation (an org's invite) can also match a second relation.
        assert!(first["held_entangled"][r.key].as_i64().unwrap_or(0) >= subjects_entangled, "{}: {first}", r.key);
        let c = &first["cross_refs"][r.key];
        assert_eq!(c["outcome"], r.outcome.as_str(), "{}", r.key);
        assert!(c["other"].as_i64().unwrap() >= subjects_entangled, "{} other-class rows: {c}", r.key);
        assert!(c["synthetic_implicit"].as_i64().unwrap() + c["synthetic_explicit"].as_i64().unwrap() >= 1, "{}: {c}", r.key);
    }
    let after_first = snapshot(&conn, &involved);
    assert!(after_first == before, "the report-only sweep changed rows:\n{}", snapshot_diff(&before, &after_first));

    // Sweep 2 deletes the unentangled subject and holds every other: no row of either tenant changes.
    server.state.db.prune_once().await.expect("second sweep");
    let second = tenp::last_block(&conn);
    assert_eq!(second["purged"], 1, "only the plain subject goes: {second}");
    assert!(!tenp::exists(&conn, plain), "the subject nobody names is purged");
    for r in &two_sided {
        assert!(second["held_entangled"][r.key].as_i64().unwrap_or(0) >= subjects_entangled, "{}: {second}", r.key);
    }
    let held_after = snapshot(&conn, &held_snapshot_ids);
    assert!(held_after == held_before, "held subjects and their owners changed:\n{}", snapshot_diff(&held_before, &held_after));
    for (_, s, _) in &class_subjects {
        assert!(tenp::exists(&conn, *s));
    }

    // The claimed tenant's rows go; the mixed subject is now named only by synthetic tenants.
    for (r, rowid) in two_sided.iter().zip(&planted_claimed) {
        match r.subject {
            Some(col) if r.table == "tenants" => {
                conn.execute(&format!("UPDATE tenants SET {col} = NULL WHERE id = ?1"), params![rowid]).unwrap();
            }
            _ => {
                let pk = "rowid";
                conn.execute(&format!("DELETE FROM \"{}\" WHERE {pk} = ?1", r.table), params![rowid]).unwrap();
            }
        }
    }
    server.state.db.prune_once().await.expect("third sweep");
    let third = tenp::last_block(&conn);
    assert_eq!(third["purged"], 1, "the mixed subject is purged: {third}");
    assert!(!tenp::exists(&conn, mix));
    for r in &two_sided {
        assert!(third["held_entangled"][r.key].as_i64().unwrap_or(0) >= OTHER_CLASSES.len() as i64, "{}: {third}", r.key);
    }
    for (r, rowid) in two_sided.iter().zip(&planted_syn) {
        let probed = probe(&conn, r, *rowid);
        match r.outcome {
            PurgeOutcome::Nulled => assert_eq!(probed, Some(true), "{}: the row stays with the column NULL", r.key),
            PurgeOutcome::Delete | PurgeOutcome::Cascade => assert_eq!(probed, None, "{}: the row goes with the subject", r.key),
            PurgeOutcome::SetNull => unreachable!(),
        }
    }
    assert_eq!(rows_naming(&conn, mix), Vec::<String>::new(), "no row names the purged subject");
    for (table, rowid, original) in &text_rows {
        let now_text: String = conn
            .query_row(&format!("SELECT {} FROM \"{}\" WHERE rowid = ?1", original.0, table), params![rowid], |r| r.get(0))
            .unwrap_or_else(|e| panic!("{table} row survives: {e}"));
        assert_eq!(&now_text, &original.1, "{table}.{} is never rewritten", original.0);
    }
    let detail: String = conn
        .query_row("SELECT detail FROM admin_events WHERE action = 'tenant_purge_synthetic' AND tenant = 'tenp-mix'", [], |r| r.get(0))
        .expect("the purge journals the subject");
    let detail: Value = serde_json::from_str(&detail).unwrap();
    assert!(detail["cross_refs"].as_object().is_some_and(|m| m.len() >= keys.len()), "the journal row carries the cross_refs: {detail}");
    let tombstone: i64 = conn
        .query_row("SELECT COUNT(*) FROM namespace_tombstones WHERE namespace = 'tenp-mix'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tombstone, 1, "the freed namespace is reserved");
}

/// Plants the text-only references: returns `(table, rowid, (column, value))` for each.
fn plant_text_columns(conn: &Connection, p: &Planter, subject: i64, ns: &str, other: i64) -> Vec<(String, i64, (String, String))> {
    let mut out = Vec::new();
    let mut add = |table: &str, rowid: i64, col: &str, value: String| out.push((table.to_string(), rowid, (col.to_string(), value)));
    let tool = format!("{ns}.tool");
    p.plant_with(conn, "runs", "tenant_id", other, 900, &[("tool_name", text(&tool))]).unwrap();
    add("runs", conn.last_insert_rowid(), "tool_name", tool);
    let forked = format!("{ns}.original");
    p.plant_with(conn, "tools", "tenant_id", other, 900, &[("forked_from", text(&forked))]).unwrap();
    add("tools", conn.last_insert_rowid(), "forked_from", forked);
    p.plant_with(conn, "messages", "from_tenant_id", subject, 900, &[("from_address", text(ns))]).unwrap();
    add("messages", conn.last_insert_rowid(), "from_address", ns.to_string());
    let refused = format!("{{\"to\":\"{ns}\"}}");
    p.plant_with(conn, "messages", "from_tenant_id", subject, 901, &[("refused_json", text(&refused))]).unwrap();
    add("messages", conn.last_insert_rowid(), "refused_json", refused);
    p.plant_with(conn, "channel_posts", "from_tenant_id", subject, 900, &[("from_address", text(ns))]).unwrap();
    add("channel_posts", conn.last_insert_rowid(), "from_address", ns.to_string());
    out
}

#[tokio::test]
async fn tenp_ac02_every_cross_reference_lookup_uses_an_index() {
    let server = TestServer::start().await;
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    for r in CROSS_REFS {
        let sql = format!("EXPLAIN QUERY PLAN {}", r.owners);
        let mut stmt = conn.prepare(&sql).unwrap_or_else(|e| panic!("{}: {e}", r.key));
        let plan: Vec<String> = stmt
            .query_map(rusqlite::named_params! {":id": 1}, |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let scans: Vec<_> = plan.iter().filter(|p| p.starts_with("SCAN ") && !p.contains("USING")).collect();
        assert!(scans.is_empty(), "{} scans a table per subject: {plan:?}", r.key);
    }
}

#[tokio::test]
async fn tenp_ac02_rows_the_subject_owns_that_name_a_claimed_tenant_are_one_sided() {
    let _env = EnvGuard::clear(tenp::BUDGET_ENV).await;
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    let now = mcphost::state::now_unix();
    tenp::set_floor(&conn, now - 40 * DAY);
    let planter = Planter::new(&conn, now - 3600);
    let subject = tenp::insert(&conn, &Row::purge_subject("tenp-onesided", now, "idle_expired", 60, 30));
    let claimed = tenp::insert(&conn, &owner_class("claimed", "tenp-claimed-friend", now));
    // The subject's own contact and call rows name the claimed tenant (a contact, a caller); the
    // claimed tenant has rows of its own in the same tables.
    planter.plant_with(&conn, "contacts", "tenant_id", subject, 1, &[("contact_tenant_id", int(claimed))]).unwrap();
    planter.plant_with(&conn, "calls", "tenant_id", subject, 1, &[("caller_tenant_id", int(claimed))]).unwrap();
    planter.plant(&conn, "contacts", "tenant_id", claimed, 2).unwrap();
    planter.plant(&conn, "calls", "tenant_id", claimed, 2).unwrap();
    let owned = |conn: &Connection| -> Vec<String> {
        ["contacts", "calls"]
            .iter()
            .flat_map(|t| {
                let mut stmt = conn.prepare(&format!("SELECT rowid, * FROM \"{t}\" WHERE tenant_id = ?1")).unwrap();
                let n = stmt.column_count();
                stmt.query_map(params![claimed], |r| {
                    let mut s = format!("{t}|");
                    for i in 0..n {
                        s.push_str(&format!("{:?}|", r.get::<_, Sql>(i)?));
                    }
                    Ok(s)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
            })
            .collect()
    };
    let claimed_before = owned(&conn);
    assert_eq!(claimed_before.len(), 2);

    server.state.db.prune_once().await.expect("report-only sweep");
    let first = tenp::last_block(&conn);
    assert_eq!(first["held_entangled"], serde_json::json!({}), "nothing is held: {first}");
    assert_eq!(first["would_purge"], 1, "{first}");
    server.state.db.prune_once().await.expect("purging sweep");
    let second = tenp::last_block(&conn);
    assert_eq!(second["purged"], 1, "{second}");
    assert!(!tenp::exists(&conn, subject), "the subject is purged");
    // Rows the subject owns went with it; rows the claimed tenant owns are the claimed tenant's.
    assert_eq!(owned(&conn), claimed_before, "the claimed tenant's own rows are unchanged");
    assert_eq!(rows_naming(&conn, subject), Vec::<String>::new());
}
