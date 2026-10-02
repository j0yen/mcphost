//! PRD-mcphost-end-user-audit-and-revoke
//! AC10 (P0) -- Given 100 000 end users, When `list` pages with
//! `limit: 100`, Then each page returns in < 50 ms with a stable cursor.
//!
//! Split into two tests (PRD-mcphost-test-suite-flake-lints requirement 6):
//! pagination correctness (completeness, no duplicate subjects, cursor
//! termination) carries no timing assertion at all, and the performance
//! half uses `perf_budget!` instead of a per-page "best of 3" sample --
//! this file's own prior "best of 3" shape still asserted a wall-clock
//! budget on a loaded shared builder (52ms once against 50ms, run 309,
//! three attempts) because it measured and asserted PER PAGE (up to 1000
//! times in one run) rather than a warm median of a few representative
//! calls, and had no `MCPHOST_PERF_SKIP` escape hatch for a loaded box.
//! `host.enduser.list` reads have no side effect, so repeated identical
//! calls at the same (absent) cursor return byte-identical pages -- the
//! same fact the old per-page retries relied on, now backing
//! `perf_budget!`'s own warm-up-then-5-timed-runs instead.

use std::collections::HashSet;

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const TOTAL_END_USERS: i64 = 100_000;
const PAGE_LIMIT: i64 = 100;
const SEED_BASE_LAST_SEEN: i64 = 2_000_000_000;

async fn seeded_tenant_client(signup_name: &str) -> (TestServer, McpClient) {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, signup_name).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    server
        .state
        .db
        .insert_end_users_bulk_for_test(tenant.id, TOTAL_END_USERS, SEED_BASE_LAST_SEEN)
        .await
        .expect("seed 100k end users");
    (server, client)
}

#[tokio::test]
async fn all_100k_end_users_are_returned_exactly_once_with_a_stable_cursor() {
    let (_server, client) = seeded_tenant_client("AC10 Correctness Tenant").await;

    let mut seen = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut args = json!({"limit": PAGE_LIMIT});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        pages += 1;

        let page = extract_structured(
            &client.tools_call("host.enduser.list", args).await.expect("host.enduser.list ok"),
        );
        let rows = page["end_users"].as_array().expect("end_users array");
        assert!(!rows.is_empty(), "page {pages} was empty before exhausting all {TOTAL_END_USERS} rows");
        for row in rows {
            let subject = row["subject"].as_str().expect("subject string").to_string();
            assert!(seen.insert(subject.clone()), "cursor is not stable: {subject} seen twice");
        }

        cursor = page["cursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
        assert!(pages <= (TOTAL_END_USERS / PAGE_LIMIT) + 1, "paging did not terminate");
    }

    assert_eq!(seen.len() as i64, TOTAL_END_USERS, "every end user must appear exactly once across pages");
    assert_eq!(pages, TOTAL_END_USERS / PAGE_LIMIT, "{pages} pages for {TOTAL_END_USERS} rows at limit {PAGE_LIMIT}");
}

#[tokio::test]
async fn a_page_fetch_against_100k_end_users_is_under_the_50ms_budget() {
    let (_server, client) = seeded_tenant_client("AC10 Perf Tenant").await;

    crate::perf_budget!(50, {
        client
            .tools_call("host.enduser.list", json!({"limit": PAGE_LIMIT}))
            .await
            .expect("host.enduser.list ok");
    });
}
