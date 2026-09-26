//! PRD-mcphost-end-user-audit-and-revoke
//! AC10 (P0) -- Given 100 000 end users, When `list` pages with
//! `limit: 100`, Then each page returns in < 50 ms with a stable cursor.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

const TOTAL_END_USERS: i64 = 100_000;
const PAGE_LIMIT: i64 = 100;
const PAGE_BUDGET: Duration = Duration::from_millis(50);

#[tokio::test]
async fn list_100k_pages_under_50ms_with_a_stable_cursor() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "AC10 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let tenant = server.state.db.find_tenant_by_namespace(ns).await.unwrap().expect("tenant");
    let base_last_seen = 2_000_000_000i64;
    server
        .state
        .db
        .insert_end_users_bulk_for_test(tenant.id, TOTAL_END_USERS, base_last_seen)
        .await
        .expect("seed 100k end users");

    let mut seen = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut args = json!({"limit": PAGE_LIMIT});
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let start = Instant::now();
        let page = extract_structured(
            &client.tools_call("host.enduser.list", args).await.expect("host.enduser.list ok"),
        );
        let elapsed = start.elapsed();
        pages += 1;
        assert!(
            elapsed < PAGE_BUDGET,
            "page {pages} took {elapsed:?}, over the {PAGE_BUDGET:?} budget"
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
