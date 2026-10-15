//! PRD-mcphost-declared-ordering-per-verb-b
//! AC2 (P0) -- Given every `Paged` verb with a fixture, When the concurrent
//! walk (`vord_ac05` in the PRD's numbering, `vord_ac06` here) runs it under
//! concurrent inserts under each declared order for 20 trials, Then zero
//! duplicate ids and zero skipped pre-existing ids.
//!
//! The all-verb walk lives in `vord_ac06`; this file pins that it cannot fall
//! behind the verb set or the declared orders, and re-runs the DB-backed
//! `host.runs.list` walk for 20 trials under every declared order.

use crate::common;
use common::{McpClient, TestServer, signup, vord_dups_and_skips, vord_seed_runs, vord_walk};
use mcphost::paged::{CallsOrder, ChannelOrder, DocsOrder, InboxOrder, OrderEnum, PAGED_VERBS, RunsOrder, ThreadOrder};
use serde_json::json;
use std::time::Duration;

#[test]
fn the_all_verb_walk_names_every_paged_verb_and_every_declared_order() {
    let walk = include_str!("vord_ac06_concurrent_inserts_every_paged_verb.rs");
    for verb in PAGED_VERBS {
        assert!(walk.contains(&format!("\"{verb}\"")), "{verb} is Paged but the concurrent walk never mentions it");
    }
    for names in [DocsOrder::names(), InboxOrder::names(), ThreadOrder::names(), ChannelOrder::names(), RunsOrder::names(), CallsOrder::names()] {
        assert!(!names.is_empty(), "every Paged verb declares at least one order");
    }
    for needle in ["const TRIALS: usize = 20;", "DocsOrder::names()", "RunsOrder::names()"] {
        assert!(walk.contains(needle), "walk must keep `{needle}`");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runs_list_walks_clean_under_concurrent_inserts_for_twenty_trials_per_order() {
    let server = std::sync::Arc::new(TestServer::start_with_signup_rate_limit(100_000).await);
    for order in RunsOrder::names() {
        for trial in 0..20 {
            let (ns, key) = signup(&server.base_url, &format!("Vordb AC2 {order} {trial}")).await;
            let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("find").expect("tenant");
            vord_seed_runs(&server, tenant.id, 60, 0).await;
            let reader = McpClient::with_bearer(&server.base_url, &key);
            let args = json!({"order": order});
            let pre = vord_walk(&reader, "host.runs.list", "runs", "run_id", args.clone(), 100, Duration::ZERO).await;
            let state = server.state.clone();
            let writer = tokio::spawn(async move {
                for _ in 0..200 {
                    state
                        .db
                        .insert_done_run_with_progress_for_test(tenant.id, "vord_echo".to_string(), json!({"budget": {"verdict": "ok"}}).to_string())
                        .await
                        .expect("write run");
                    tokio::task::yield_now().await;
                }
            });
            let walked = vord_walk(&reader, "host.runs.list", "runs", "run_id", args, 25, Duration::from_millis(2)).await;
            writer.await.expect("writer");
            assert_eq!(vord_dups_and_skips(&walked, &pre), (0, 0), "order={order} trial={trial}");
        }
    }
}
