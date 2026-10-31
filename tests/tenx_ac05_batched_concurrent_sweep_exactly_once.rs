//! PRD-mcphost-synthetic-implicit-tenant-expiry
//! AC5 (P0) — Given 6,000 subjects and 10 non-subjects, When `prune_once` runs
//! twice concurrently and once more on an empty set, Then every subject is
//! expired exactly once over more than one batch, a subject touched by
//! `touch_last_seen` between batches is skipped, non-subjects are unchanged,
//! each run writes one `prune_log` row (the one that swept carries `expired`,
//! `skipped_billing`, `would_expire`, 0 and no error for the empty run; the
//! other reports `busy`, PRD-mcphost-synthetic-tenant-purge R1), and no
//! teardown ran twice. A row minted before the floor is a subject too (R2).

use crate::common::tenx::{self, DAY, Fx};
use crate::common::{EnvGuard, TestServer};
use async_trait::async_trait;
use mcphost::kinds::{Kind, KindError, KindRegistry, ToolDescriptor};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const SUBJECTS: usize = 6_000;

/// Records each removed tenant id; on the first one (so after batch 1 was
/// claimed) it touches `victim`, the subject that must therefore be skipped.
struct HookKind {
    removed: Arc<Mutex<Vec<i64>>>,
    db: Arc<OnceLock<mcphost::db::Db>>,
    victim: Arc<AtomicI64>,
    touched: AtomicBool,
}

#[async_trait]
impl Kind for HookKind {
    fn contract(&self) -> &'static mcphost::kinds::contract::KindContract {
        &mcphost::kinds::contract::EMPTY_CONTRACT
    }
    fn name(&self) -> &'static str {
        "tenxhook"
    }
    fn validate(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }
    fn describe(&self, _spec: &Value) -> ToolDescriptor {
        ToolDescriptor {
            name: "tenxhook".to_string(),
            description: "records tenant removal".to_string(),
            input_schema: json!({"type": "object"}),
        }
    }
    async fn call(&self, _spec: &Value, _args: Value, _ctx: &mcphost::kinds::CallCtx) -> Result<Value, KindError> {
        Ok(json!({}))
    }
    async fn on_tenant_removed(&self, tenant_id: i64) {
        self.removed.lock().unwrap().push(tenant_id);
        if !self.touched.swap(true, Ordering::SeqCst) {
            let db = self.db.get().expect("db installed");
            db.touch_last_seen(self.victim.load(Ordering::SeqCst), mcphost::state::now_unix())
                .await
                .expect("touch between batches");
        }
    }
}

fn log_rows(conn: &rusqlite::Connection) -> Vec<(bool, serde_json::Map<String, Value>)> {
    let mut stmt = conn.prepare("SELECT ok, deleted_json FROM prune_log ORDER BY id").unwrap();
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, String>(1)?)))
        .unwrap()
        .map(|row| {
            let (ok, json) = row.unwrap();
            (ok, serde_json::from_str::<Value>(&json).unwrap().as_object().unwrap().clone())
        })
        .collect()
}

#[tokio::test]
async fn tenx_ac05_concurrent_sweeps_expire_every_subject_exactly_once() {
    let _env = EnvGuard::clear(tenx::ENV).await;
    let removed = Arc::new(Mutex::new(Vec::new()));
    let db_cell = Arc::new(OnceLock::new());
    let victim = Arc::new(AtomicI64::new(0));
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HookKind {
        removed: removed.clone(),
        db: db_cell.clone(),
        victim: victim.clone(),
        touched: AtomicBool::new(false),
    }));
    let server = TestServer::start_with_kinds(kinds).await;
    db_cell.set(server.state.db.clone()).ok();
    server.state.db.seed_retention_policy_from_env().await.expect("seed");
    let mut conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("raw db");
    let now = mcphost::state::now_unix();
    tenx::set_floor(&conn, now - 10 * DAY);

    let idle = 2 * DAY;
    let mut subjects = Vec::with_capacity(SUBJECTS);
    let tx = conn.transaction().unwrap();
    // The first row, so the touched subject (the last row) stays the only one in the last batch.
    // Minted before the floor: its idle run starts at the floor (R2), so it is a subject.
    let mut before_floor = Fx::subject("tenx-s-before-floor", now, 11, idle);
    before_floor.created_unix = now - 11 * DAY;
    let before_floor = tenx::insert(&tx, &before_floor);
    for i in 0..SUBJECTS {
        subjects.push(tenx::insert(&tx, &Fx::subject(&format!("tenx-s{i}"), now, 3, idle)));
    }
    let mut non_subjects = Vec::new();
    let mut add = |label: &str, mutate: &dyn Fn(&mut Fx)| {
        let mut fx = Fx::subject(&format!("tenx-n-{label}"), now, 3, idle);
        mutate(&mut fx);
        non_subjects.push(tenx::insert(&tx, &fx));
    };
    add("claimed", &|f| f.claimed_unix = Some(now - DAY));
    add("external", &|f| f.origin = "external");
    add("idle-1h", &|f| f.last_seen_unix = Some(now - 3600));
    add("paid", &|f| f.paid_unix = Some(now - DAY));
    add("synthetic-null", &|f| f.synthetic = None);
    add("billing-ref", &|f| f.billing_ref = Some("cus_test"));
    add("explicit", &|f| f.signup_source = Some("explicit"));
    add("unclassified", &|f| f.origin = "unclassified");
    add("source-null", &|f| f.signup_source = None);
    assert_eq!(non_subjects.len(), 9);
    tx.commit().unwrap();
    // The last-inserted subject: the claim walks rowids upward, so it is in the last batch.
    let target = *subjects.last().unwrap();
    victim.store(target, Ordering::SeqCst);

    let (a, b) = tokio::join!(server.state.db.prune_once(), server.state.db.prune_once());
    a.expect("first concurrent run");
    b.expect("second concurrent run");

    let ids = removed.lock().unwrap().clone();
    let unique: HashSet<i64> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len(), "no teardown ran twice");
    assert_eq!(unique.len(), SUBJECTS, "every subject but the touched one, and the pre-floor row, was torn down");
    assert!(!unique.contains(&target));
    assert!(unique.contains(&before_floor));
    for id in &subjects[..SUBJECTS - 1] {
        assert_eq!(tenx::state(&conn, *id), (true, Some("idle_expired".to_string())), "subject {id}");
    }
    assert_eq!(tenx::state(&conn, target), (false, None), "the touched subject was skipped");
    for id in &non_subjects {
        assert_eq!(tenx::state(&conn, *id), (false, None), "non-subject {id} unchanged");
    }

    // PRD-mcphost-synthetic-tenant-purge R1: the sweeps share one mutex, so the second
    // finds the first running and logs `busy` instead of claiming alongside it.
    let rows = log_rows(&conn);
    assert_eq!(rows.len(), 2, "one prune_log row per run");
    let key = |k: &str| format!("synthetic_implicit_tenants.{k}");
    let busy = |d: &serde_json::Map<String, Value>| d["synthetic_implicit_tenants"]["busy"] == json!(true);
    assert_eq!(rows.iter().filter(|(_, d)| busy(d)).count(), 1, "exactly one run found the other: {rows:?}");
    let (ok, worked) = rows.iter().find(|(_, d)| !busy(d)).expect("the run that swept");
    assert!(*ok);
    assert_eq!(worked[&key("expired")].as_i64(), Some(SUBJECTS as i64), "the sweep claimed each subject once");
    // Not yet idle the window: the idle-1h row and the touched subject.
    assert_eq!((worked[&key("skipped_billing")].as_i64(), worked[&key("would_expire")].as_i64()), (Some(1), Some(2)));
    assert!(rows.iter().all(|(ok, _)| *ok));

    // An empty set: nothing left to claim, no backlog.
    conn.execute_batch("DELETE FROM tenants").expect("empty the table");
    server.state.db.prune_once().await.expect("empty run");
    let rows = log_rows(&conn);
    assert_eq!(rows.len(), 3);
    let (ok, last) = rows.last().unwrap();
    assert!(*ok, "no error on an empty set");
    assert_eq!(
        (last[&key("expired")].as_i64(), last[&key("skipped_billing")].as_i64(), last[&key("would_expire")].as_i64()),
        (Some(0), Some(0), Some(0))
    );
    assert_eq!(removed.lock().unwrap().len(), SUBJECTS, "the empty run tore nothing down");
}
