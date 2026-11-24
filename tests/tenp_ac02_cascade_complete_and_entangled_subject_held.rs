//! PRD-mcphost-synthetic-tenant-purge
//! AC2 (P0) — Given a subject with a row in every table naming tenants by key
//! or name (enumerated at test time; two-sided set = § Cascade) and, per
//! two-sided column, a synthetic and a claimed two-marker tenant's row naming
//! it, When sweeps run, Then it is held, `held_entangled` names each
//! claimed-owned column, no row of either tenant changes; claimed rows gone, it
//! is purged, synthetic rows get their § Cascade outcome and no row names it.
//!
//! Also R3: the schema-derived two-sided set equals the registry the purge reads
//! (a table added later fails here by name), the other-class plants repeat for a
//! label-only and an external tenant, a subject-owned row naming a claimed tenant
//! is one-sided, text columns that carry the namespace are never rewritten, and the
//! same-tenant members hold only their own tenant's rows after their real INSERT path.

use crate::common::tenp::{self, DAY, Row};
use crate::common::{McpClient, TestServer, oauth_consent_and_token, signup};
use axum::{Json, Router, routing::post};
use mcphost::db::synthetic_purge::{Outcome, RELATIONS, SAME_TENANT};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// One step that plants the other side of a relation: a row (set based over
/// `tenp_ids`, `id` the subject, `other` the other tenant) or a statement.
enum Step {
    Plant(&'static str, &'static [(&'static str, &'static str)]),
    Sql(&'static str),
}
use Step::{Plant, Sql};

/// How to plant, and how to remove again, the row of one relation.
struct Recipe {
    key: &'static str,
    steps: &'static [Step],
    /// Statement that removes the other tenant's row; `None` deletes the last planted row.
    undo: Option<&'static str>,
}

const NEWEST_GROUP: &str = "(SELECT MAX(id) FROM groups)";
const NEWEST_ORG: &str = "(SELECT MAX(id) FROM orgs)";
const NEWEST_INVITE: &str = "(SELECT MAX(id) FROM invites)";
const NEWEST_CHANNEL: &str = "(SELECT id FROM channels ORDER BY rowid DESC LIMIT 1)";

const RECIPES: &[Recipe] = &[
    Recipe {
        key: "calls.caller_tenant_id",
        steps: &[Plant("calls", &[("tenant_id", "other"), ("caller_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "runs.caller_tenant_id",
        steps: &[Plant("runs", &[("tenant_id", "other"), ("caller_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "tenants.invited_by_tenant_id",
        steps: &[Sql("UPDATE tenants SET invited_by_tenant_id = {S} WHERE id = {O}")],
        undo: Some("UPDATE tenants SET invited_by_tenant_id = NULL WHERE id = {O}"),
    },
    Recipe {
        key: "tenants.scratch_of",
        steps: &[Sql("UPDATE tenants SET scratch_of = {S} WHERE id = {O}")],
        undo: Some("UPDATE tenants SET scratch_of = NULL WHERE id = {O}"),
    },
    Recipe {
        key: "usage_daily.caller_tenant_id",
        steps: &[Plant("usage_daily", &[("tenant_id", "other"), ("caller_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "blocks.blocked_tenant_id",
        steps: &[Plant("blocks", &[("tenant_id", "other"), ("blocked_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "contacts.contact_tenant_id",
        steps: &[Plant("contacts", &[("tenant_id", "other"), ("contact_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "muted.muted_tenant_id",
        steps: &[Plant("muted", &[("tenant_id", "other"), ("muted_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "tool_version_watermarks.caller_tenant_id",
        steps: &[Plant("tool_version_watermarks", &[("tenant_id", "other"), ("caller_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "tool_caller_limits.caller_tenant_id",
        steps: &[Plant("tool_caller_limits", &[("tenant_id", "other"), ("caller_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "contact_requests.from_tenant_id",
        steps: &[Plant("contact_requests", &[("from_tenant_id", "id"), ("to_tenant_id", "other")])],
        undo: None,
    },
    Recipe {
        key: "contact_requests.to_tenant_id",
        steps: &[Plant("contact_requests", &[("from_tenant_id", "other"), ("to_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "tasks.poster_tenant_id",
        steps: &[Plant("tasks", &[("poster_tenant_id", "id"), ("recipient_tenant_id", "other")])],
        undo: None,
    },
    Recipe {
        key: "tasks.recipient_tenant_id",
        steps: &[Plant("tasks", &[("poster_tenant_id", "other"), ("recipient_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "invite_joins.inviter_tenant_id",
        steps: &[Plant("invite_joins", &[("inviter_tenant_id", "id"), ("invitee_tenant_id", "other")])],
        undo: None,
    },
    Recipe {
        key: "invite_joins.invitee_tenant_id",
        steps: &[Plant("invite_joins", &[("inviter_tenant_id", "other"), ("invitee_tenant_id", "id")])],
        undo: None,
    },
    Recipe {
        key: "groups.owner_tenant_id/group_members.member_tenant_id",
        steps: &[
            Plant("groups", &[("owner_tenant_id", "id")]),
            Plant("group_members", &[("group_id", NEWEST_GROUP), ("member_tenant_id", "other")]),
        ],
        undo: None,
    },
    Recipe {
        key: "groups.owner_tenant_id/channels.created_by",
        steps: &[
            Plant("groups", &[("owner_tenant_id", "id")]),
            Plant("channels", &[("group_id", NEWEST_GROUP), ("created_by", "other")]),
        ],
        undo: None,
    },
    Recipe {
        key: "groups.owner_tenant_id/channel_posts.from_tenant_id",
        steps: &[
            Plant("groups", &[("owner_tenant_id", "id")]),
            Plant("channels", &[("group_id", NEWEST_GROUP), ("created_by", "NULL")]),
            Plant("channel_posts", &[("channel_id", NEWEST_CHANNEL), ("from_tenant_id", "other")]),
        ],
        undo: None,
    },
    Recipe {
        key: "groups.owner_tenant_id/channel_cursors.tenant_id",
        steps: &[
            Plant("groups", &[("owner_tenant_id", "id")]),
            Plant("channels", &[("group_id", NEWEST_GROUP), ("created_by", "NULL")]),
            Plant("channel_cursors", &[("channel_id", NEWEST_CHANNEL), ("tenant_id", "other")]),
        ],
        undo: None,
    },
    Recipe {
        key: "orgs.owner_tenant_id/org_members.tenant_id",
        steps: &[
            Plant("orgs", &[("owner_tenant_id", "id")]),
            Plant("org_members", &[("org_id", NEWEST_ORG), ("tenant_id", "other")]),
        ],
        undo: None,
    },
    Recipe {
        key: "orgs.owner_tenant_id/invites.inviter_tenant_id",
        steps: &[
            Plant("orgs", &[("owner_tenant_id", "id")]),
            Plant("invites", &[("org_id", NEWEST_ORG), ("inviter_tenant_id", "other")]),
        ],
        undo: None,
    },
    Recipe {
        key: "orgs.owner_tenant_id/invite_joins.invitee_tenant_id",
        steps: &[
            Plant("orgs", &[("owner_tenant_id", "id")]),
            Plant("invites", &[("org_id", NEWEST_ORG), ("inviter_tenant_id", "id")]),
            Plant(
                "invite_joins",
                &[("invite_id", NEWEST_INVITE), ("inviter_tenant_id", "id"), ("invitee_tenant_id", "other")],
            ),
        ],
        undo: None,
    },
    Recipe {
        key: "invites.inviter_tenant_id/invite_joins.invitee_tenant_id",
        steps: &[
            Plant("invites", &[("org_id", "NULL"), ("inviter_tenant_id", "id")]),
            Plant(
                "invite_joins",
                &[("invite_id", NEWEST_INVITE), ("inviter_tenant_id", "id"), ("invitee_tenant_id", "other")],
            ),
        ],
        undo: None,
    },
];

/// Where one planted row sits, so it can be checked and removed again.
struct Planted {
    table: &'static str,
    rowid: Option<i64>,
}

fn run_recipe(conn: &Connection, recipe: &Recipe, subject: i64, other: i64) -> Planted {
    let mut last = Planted { table: "", rowid: None };
    for step in recipe.steps {
        match step {
            Plant(table, assign) => {
                tenp::set_ids(conn, &[(subject, other)]);
                tenp::plant(conn, table, assign);
                last = Planted { table, rowid: Some(conn.last_insert_rowid()) };
            }
            Sql(sql) => {
                let sql = sql.replace("{S}", &subject.to_string()).replace("{O}", &other.to_string());
                assert_eq!(conn.execute(&sql, []).unwrap(), 1, "{sql}");
                last = Planted { table: "tenants", rowid: None };
            }
        }
    }
    last
}

fn undo_recipe(conn: &Connection, recipe: &Recipe, planted: &Planted, other: i64) {
    match (recipe.undo, planted.rowid) {
        (Some(sql), _) => {
            conn.execute(&sql.replace("{O}", &other.to_string()), []).unwrap();
        }
        (None, Some(rowid)) => {
            conn.execute(&format!("DELETE FROM \"{}\" WHERE rowid = ?1", planted.table), params![rowid]).unwrap();
        }
        (None, None) => panic!("{}: nothing to undo", recipe.key),
    }
}

fn live(ns: &str, now: i64) -> Row {
    Row { disabled: false, reason: None, ..Row::subject(ns, now) }
}

/// The five tenants a subject can be named by.
struct Others {
    synthetic_implicit: i64,
    synthetic_explicit: i64,
    claimed: i64,
    label_only: i64,
    external: i64,
}

fn add_others(conn: &Connection, now: i64) -> Others {
    Others {
        synthetic_implicit: tenp::insert(conn, &live("tenp-o-implicit", now)),
        synthetic_explicit: tenp::insert(
            conn,
            &Row { signup_source: Some("explicit"), ..live("tenp-o-explicit", now) },
        ),
        claimed: tenp::insert(conn, &Row { claimed_unix: Some(now), ..live("tenp-o-claimed", now) }),
        label_only: tenp::insert(conn, &Row { origin: "unclassified", ..live("tenp-o-label", now) }),
        external: tenp::insert(
            conn,
            &Row { origin: "external", synthetic: None, signup_source: Some("explicit"), ..live("tenp-o-external", now) },
        ),
    }
}

/// One row in every table that names a tenant, all naming `subject`.
fn plant_everywhere(conn: &Connection, subject: i64) {
    tenp::set_ids(conn, &[(subject, subject)]);
    for table in tenp::tenant_tables(conn).keys() {
        if table != "tenants" && table != "tenant_lifecycle_events" {
            tenp::plant(conn, table, &[]);
        }
    }
}

fn namespace(conn: &Connection, id: i64) -> String {
    conn.query_row("SELECT namespace FROM tenants WHERE id = ?1", params![id], |r| r.get(0)).unwrap()
}

/// Every cross-tenant relation a purge can hold a subject on.
fn held_keys() -> BTreeSet<&'static str> {
    RELATIONS.iter().filter(|r| r.outcome != Outcome::SetNull).map(|r| r.key).collect()
}

#[test]
fn tenp_ac02_registry_equals_the_schema_derived_two_sided_set() {
    let dir = crate::common::TempDataDir::new();
    let db = mcphost::db::Db::open(&dir.0).expect("open");
    drop(db);
    let conn = tenp::open_raw(&dir.0);
    let derived = tenp::derive_two_sided(&conn);

    let relation_keys: BTreeSet<String> = RELATIONS.iter().map(|r| r.key.to_string()).collect();
    assert_eq!(relation_keys.len(), RELATIONS.len(), "relation keys are unique");
    // Flat (a) and (b): the tables of the registry's non-structural, non-SET NULL columns.
    let flat_registry: BTreeSet<String> = RELATIONS
        .iter()
        .filter(|r| r.outcome != Outcome::SetNull && !r.key.contains('/'))
        .map(|r| r.key.split('.').next().unwrap().to_string())
        .collect();
    assert_eq!(derived.flat_tables, flat_registry, "(a) and (b): a table with two tenant columns is classified");
    // Structural (c): parent/child keys, registry plus the same-tenant members.
    let structural_registry: BTreeSet<String> = RELATIONS
        .iter()
        .filter(|r| r.key.contains('/'))
        .map(|r| r.key.to_string())
        .chain(SAME_TENANT.iter().map(|k| k.to_string()))
        .collect();
    assert_eq!(derived.structural_keys, structural_registry, "(c): a cascading child table is classified");
    let structural_tables: BTreeSet<String> = structural_registry
        .iter()
        .map(|k| k.split_once('/').unwrap().1.split('.').next().unwrap().to_string())
        .collect();
    assert_eq!(derived.structural_tables, structural_tables);
    assert_eq!(derived.structural_tables.len(), 9, "7 cross-tenant and 2 same-tenant members on origin/main");

    // Every registry column exists, and every tenant column of the schema is a
    // column of a classified table, a SET NULL author column, or one-sided.
    let all: BTreeMap<String, Vec<tenp::TenantCol>> = tenp::tenant_tables(&conn);
    for r in RELATIONS {
        let (parent, child) = r.key.split_once('/').unwrap_or((r.key, r.key));
        for side in [parent, child] {
            let (table, column) = side.split_once('.').expect("table.column");
            assert!(
                all.get(table).is_some_and(|cols| cols.iter().any(|c| c.column == column)),
                "{} names {table}.{column}, which is not a tenant column of the schema",
                r.key
            );
        }
    }
    let recipe_keys: BTreeSet<&str> = RECIPES.iter().map(|r| r.key).collect();
    assert_eq!(recipe_keys, held_keys(), "every holding relation has a plant recipe");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenp_ac02_entangled_subject_is_held_then_purged_once_the_claimed_rows_are_gone() {
    let server = TestServer::start().await;
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let conn = tenp::open_raw(&server.data_dir.0);
    let now = mcphost::state::now_unix();
    let last_seen = now - 100 * DAY;
    crate::common::tenx::set_floor(&conn, last_seen);
    tenp::arm_gate(&conn, now);

    // Same-tenant members: their real INSERT paths write only their own tenant.
    let (_ns, key) = signup(&server.base_url, "Tenp Notify").await;
    let agent = McpClient::with_bearer(&server.base_url, &key);
    let seen = {
        let sink = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let counter = sink.clone();
        let router = Router::new().route(
            "/hook",
            post(move |Json(_): Json<Value>| {
                let counter = counter.clone();
                async move {
                    *counter.lock().unwrap() += 1;
                    "ok"
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        agent
            .tools_call("host.notify.channel_add", json!({"kind": "webhook", "target": format!("http://{addr}/hook")}))
            .await
            .expect("channel_add");
        agent.tools_call("host.notify.send", json!({"text": "tenp", "level": "info"})).await.expect("send");
        sink
    };
    drop(seen);
    let (_client, token) =
        oauth_consent_and_token(&server.base_url, &format!("{}/mcp", server.base_url), &[("new_workspace", "1")]).await;
    assert!(token["access_token"].is_string(), "{token}");
    for (child, parent, link) in [
        ("notify_receipts", "notifies", "notify_id"),
        ("oauth_jti_denylist", "oauth_grants", "grant_id"),
    ] {
        let total = tenp::count(&conn, &format!("SELECT COUNT(*) FROM {child}"));
        assert!(total >= 1, "{child}: the real INSERT path wrote a row");
        let crossed = tenp::count(
            &conn,
            &format!("SELECT COUNT(*) FROM {child} c JOIN {parent} p ON p.id = c.{link} WHERE c.tenant_id != p.tenant_id"),
        );
        assert_eq!(crossed, 0, "{child} rows carry their parent's tenant");
    }

    // The subjects and the tenants that name them.
    let sa = tenp::insert(&conn, &Row::subject("tenp-subject-a", last_seen));
    let sd = tenp::insert(&conn, &Row::subject("tenp-subject-d", last_seen));
    let sc = tenp::insert(&conn, &Row::subject("tenp-subject-c", last_seen));
    let others = add_others(&conn, now);
    let sa_ns = namespace(&conn, sa);

    plant_everywhere(&conn, sa);
    plant_everywhere(&conn, sd);
    // R3 (d): text columns that carry the namespace, never rewritten. Pair `(sa, 0)` keeps
    // their generated keys apart from every other planted row.
    let implicit = others.synthetic_implicit.to_string();
    tenp::set_ids(&conn, &[(sa, 0)]);
    tenp::plant(
        &conn,
        "runs",
        &[("tenant_id", &implicit), ("caller_tenant_id", "NULL"), ("tool_name", &format!("'{sa_ns}.shared'"))],
    );
    tenp::plant(&conn, "tools", &[("tenant_id", &implicit), ("forked_from", &format!("'{sa_ns}.origin'"))]);
    tenp::plant(&conn, "messages", &[("from_address", &format!("'{sa_ns}'")), ("refused_json", &format!("'[\"{sa_ns}\"]'"))]);
    tenp::plant(&conn, "channel_posts", &[("from_address", &format!("'{sa_ns}'"))]);
    let text_before = text_columns(&conn, &sa_ns);
    assert_eq!(text_before.len(), 5, "{text_before:?}");

    // SA: named by a synthetic implicit, a synthetic explicit and a claimed tenant.
    let mut planted_claimed: Vec<(&Recipe, Planted)> = Vec::new();
    let mut planted_synthetic: Vec<(&Recipe, i64, Planted)> = Vec::new();
    for recipe in RECIPES {
        for other in [others.synthetic_implicit, others.synthetic_explicit] {
            planted_synthetic.push((recipe, other, run_recipe(&conn, recipe, sa, other)));
        }
        planted_claimed.push((recipe, run_recipe(&conn, recipe, sa, others.claimed)));
    }
    // SD: named by a label-only and an external tenant (both `other`).
    for recipe in RECIPES {
        run_recipe(&conn, recipe, sd, others.label_only);
        run_recipe(&conn, recipe, sd, others.external);
    }
    // SC: its own rows name the claimed tenant (one-sided) and nothing names SC.
    plant_one_sided(&conn, sc, others.claimed);
    conn.execute_batch("UPDATE documents SET seq = 0; UPDATE index_sources SET paused = 1").unwrap();

    let all_ids = [sa, sd, others.synthetic_implicit, others.synthetic_explicit, others.claimed, others.label_only, others.external];
    // Rows the sweep's other jobs rewrite on their own: the usage rollup, the catalog receipts
    // cache, the blob sweep (`blobs.missing`) and the expired idempotency results.
    let skip = ["usage_daily", "catalog_receipts", "blobs", "call_results"];
    let before = tenp::snapshot(&conn, &all_ids, &[sc], &skip);
    let sd_group = [sd, others.label_only, others.external];
    let sd_before = tenp::snapshot(&conn, &sd_group, &[sa], &skip);
    assert!(before.lines().count() > 200, "a substantial fixture: {}", before.lines().count());

    // Sweep 1: SC (one-sided) goes; SA and SD are held.
    let first = server.state.db.prune_once().await.expect("sweep 1").synthetic;
    assert_eq!(first["purged"], 1, "{first}");
    assert_eq!(first["would_purge"], 3, "{first}");
    let held_names: BTreeSet<&str> = first["held_entangled"].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(held_names, held_keys(), "held_entangled names each two-sided column: {first}");
    for (column, subjects) in first["held_entangled"].as_object().unwrap() {
        assert_eq!(subjects, 2, "{column}: SA and SD");
        let counts = &first["cross_refs"][column];
        assert!(counts["other"].as_i64().unwrap() >= 3, "{column}: claimed + label-only + external: {counts}");
        assert!(counts["synthetic_implicit"].as_i64().unwrap() >= 1, "{column}: {counts}");
        assert!(counts["synthetic_explicit"].as_i64().unwrap() >= 1, "{column}: {counts}");
    }
    for column in RELATIONS.iter().filter(|r| r.outcome == Outcome::SetNull).map(|r| r.key) {
        let counts = &first["cross_refs"][column];
        assert_eq!(counts["outcome"], "set_null", "{column} is counted: {first}");
        assert!(counts["other"].is_null() && counts["rows"].as_i64().unwrap() >= 1, "{column}: {counts}");
        assert!(first["held_entangled"].get(column).is_none(), "{column} never holds");
    }
    assert!(!tenp::tenant_exists(&conn, sc), "the one-sided subject was purged");
    for id in [sa, sd] {
        assert!(tenp::tenant_exists(&conn, id), "entangled subjects are held, not purged");
    }
    let changed = tenp::diff_lines(&before, &tenp::snapshot(&conn, &all_ids, &[sc], &skip));
    assert!(changed.is_empty(), "no row of either tenant changed:\n{changed}");
    assert_eq!(tenp::rows_naming(&conn, "contacts", "tenant_id", sc), 0);
    assert!(tenp::tenant_exists(&conn, others.claimed), "the claimed tenant's own rows survive SC's purge");
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM admin_events WHERE action = 'tenant_purge_synthetic'"), 1);

    // The claimed tenant's rows that name SA are gone: SA is purged, SD stays held.
    for (recipe, planted) in &planted_claimed {
        undo_recipe(&conn, recipe, planted, others.claimed);
    }
    // Control rows: each synthetic tenant's own row in every table a relation plants into,
    // naming only itself. The purge of SA must leave them alone.
    let mut controls: Vec<(&str, i64)> = Vec::new();
    for other in [others.synthetic_implicit, others.synthetic_explicit] {
        let mut tables = BTreeSet::new();
        for (_, _, planted) in planted_synthetic.iter().filter(|(_, o, _)| *o == other) {
            // `org_members.tenant_id` is UNIQUE: a tenant has one membership, already planted.
            if planted.rowid.is_some() && planted.table != "org_members" && tables.insert(planted.table) {
                tenp::set_ids(&conn, &[(other, other)]);
                tenp::plant(&conn, planted.table, &[]);
                controls.push((planted.table, conn.last_insert_rowid()));
            }
        }
    }
    let second = server.state.db.prune_once().await.expect("sweep 2").synthetic;
    assert_eq!(second["purged"], 1, "{second}");
    assert!(!tenp::tenant_exists(&conn, sa), "SA is purged once no `other` row names it");
    assert!(tenp::tenant_exists(&conn, sd), "SD is still named by a label-only and an external tenant");
    let still_held: BTreeSet<&str> = second["held_entangled"].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(still_held, held_keys(), "{second}");
    assert!(second["held_entangled"].as_object().unwrap().values().all(|n| *n == 1));
    assert_eq!(tenp::count(&conn, "SELECT COUNT(*) FROM admin_events WHERE action = 'tenant_purge_synthetic'"), 2);

    // No row names SA, in any column that names a tenant.
    for (table, cols) in tenp::tenant_tables(&conn) {
        for c in cols {
            assert_eq!(tenp::rows_naming(&conn, &table, &c.column, sa), 0, "{table}.{} still names the purged subject", c.column);
        }
    }
    // The synthetic tenants' rows got their § Cascade outcome (below); their other rows are unchanged.
    for (table, rowid) in &controls {
        let left = tenp::count(&conn, &format!("SELECT COUNT(*) FROM \"{table}\" WHERE rowid = {rowid}"));
        assert_eq!(left, 1, "{table}: a synthetic tenant's row that never named SA survives the purge");
    }
    // SD and the tenants that name it are untouched by both sweeps.
    let changed = tenp::diff_lines(&sd_before, &tenp::snapshot(&conn, &sd_group, &[sa], &skip));
    assert!(changed.is_empty(), "the held subject's rows never change:\n{changed}");
    assert!(tenp::tenant_exists(&conn, others.synthetic_implicit) && tenp::tenant_exists(&conn, others.synthetic_explicit));
    for (recipe, other, planted) in &planted_synthetic {
        let outcome = RELATIONS.iter().find(|r| r.key == recipe.key).unwrap().outcome;
        let column = recipe.key.split_once('.').unwrap().1;
        match (outcome, planted.rowid) {
            (Outcome::Nulled, Some(rowid)) => {
                let value: Option<i64> = conn
                    .query_row(&format!("SELECT \"{column}\" FROM \"{}\" WHERE rowid = ?1", planted.table), params![rowid], |r| r.get(0))
                    .unwrap_or_else(|e| panic!("{}: the row naming SA survives: {e}", recipe.key));
                assert!(value.is_none(), "{}: the reference is NULLed", recipe.key);
            }
            (Outcome::Nulled, None) => {
                let table = recipe.key.split('.').next().unwrap();
                let column = recipe.key.split_once('.').unwrap().1;
                let value: Option<i64> = conn
                    .query_row(&format!("SELECT \"{column}\" FROM {table} WHERE id = ?1"), params![other], |r| r.get(0))
                    .unwrap_or_else(|e| panic!("{}: the tenant row survives: {e}", recipe.key));
                assert!(value.is_none(), "{}: the reference is NULLed", recipe.key);
            }
            (Outcome::Delete | Outcome::Cascade, Some(rowid)) => {
                let left = tenp::count(&conn, &format!("SELECT COUNT(*) FROM \"{}\" WHERE rowid = {rowid}", planted.table));
                assert_eq!(left, 0, "{}: the row naming SA is gone ({outcome:?})", recipe.key);
            }
            (outcome, rowid) => panic!("{}: unexpected {outcome:?} with {rowid:?}", recipe.key),
        }
    }
    let text_after = text_columns(&conn, &sa_ns);
    assert_eq!(text_after, text_before, "a text column that carries the namespace is never rewritten");
    let audit: String = conn
        .query_row("SELECT detail FROM admin_events WHERE action = 'tenant_purge_synthetic' AND tenant = ?1", params![sa_ns], |r| r.get(0))
        .expect("SA's audit row");
    let detail: Value = serde_json::from_str(&audit).unwrap();
    assert!(detail["cross_refs"].is_object(), "the audit row carries the subject's cross_refs: {detail}");
}

/// The rows whose text column carries `ns`, as `table.column -> value`.
fn text_columns(conn: &Connection, ns: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (table, column) in [
        ("runs", "tool_name"),
        ("tools", "forked_from"),
        ("messages", "from_address"),
        ("messages", "refused_json"),
        ("channel_posts", "from_address"),
    ] {
        let value: Option<String> = conn
            .query_row(
                &format!("SELECT \"{column}\" FROM {table} WHERE instr(\"{column}\", ?1) > 0 LIMIT 1"),
                params![ns],
                |r| r.get(0),
            )
            .ok();
        if let Some(v) = value {
            out.insert(format!("{table}.{column}"), v);
        }
    }
    out
}

/// Rows owned by `subject` that name `other`: one-sided, they leave with the subject.
fn plant_one_sided(conn: &Connection, subject: i64, other: i64) {
    for (table, col) in [
        ("calls", "caller_tenant_id"),
        ("runs", "caller_tenant_id"),
        ("usage_daily", "caller_tenant_id"),
        ("blocks", "blocked_tenant_id"),
        ("contacts", "contact_tenant_id"),
        ("muted", "muted_tenant_id"),
        ("tool_version_watermarks", "caller_tenant_id"),
        ("tool_caller_limits", "caller_tenant_id"),
    ] {
        tenp::set_ids(conn, &[(subject, other)]);
        tenp::plant(conn, table, &[("tenant_id", "id"), (col, "other")]);
    }
    conn.execute("UPDATE tenants SET invited_by_tenant_id = ?2, scratch_of = ?2 WHERE id = ?1", params![subject, other]).unwrap();
}
