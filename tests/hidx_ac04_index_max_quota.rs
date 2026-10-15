//! PRD-mcphost-index-create-from-sources
//! AC4 -- Given a free tenant with 3 indexes, When a fourth is created,
//! Then `quota_index_max` is returned with `data.limit: 3`, and
//! `host.quickstart.limits.plan.index_max` reads `3` from the same `Plan`
//! field.

use crate::common;
use mcphost::errors::AppError;
use mcphost::index;
use serde_json::json;

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hidx-ac04-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn create_args(name: &str) -> serde_json::Value {
    json!({"name": name, "sources": [{"kind": "doc", "prefix": "hb/"}]})
}

#[tokio::test]
async fn a_fourth_index_on_the_free_plan_is_quota_index_max_with_the_quickstart_number() {
    let dir = scratch_dir();
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hidx-ac04").await;
    assert_eq!(tenant.plan, "free");

    for name in ["one", "two", "three"] {
        index::index_create(&state, &tenant, &create_args(name)).await.expect("index within the ceiling");
    }
    let err = index::index_create(&state, &tenant, &create_args("four"))
        .await
        .expect_err("the fourth index must be refused");
    let AppError::Structured { code, data, .. } = &err else { panic!("expected a structured error, got {err:?}") };
    assert_eq!(*code, "quota_index_max");
    assert_eq!(data["limit"], json!(3), "{data}");
    assert_eq!(data["used"], json!(3), "{data}");

    let plan = state.plans.get("free").expect("free plan");
    assert_eq!(plan.index_max, 3, "the refusal's limit is the Plan field");
    assert_eq!(data["limit"], json!(plan.index_max));
    // A duplicate name is reported as such even at the ceiling, and an
    // index already created is untouched by the refused fourth.
    let dup = index::index_create(&state, &tenant, &create_args("one")).await.expect_err("duplicate");
    assert_eq!(dup.code(), "index_exists");
    let status = index::index_status(&state, &tenant, &json!({"name": "one"})).await.expect("status");
    assert_eq!(status["pending"], json!(1));

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn quickstart_limits_plan_index_max_reads_the_plan_field() {
    let server = common::TestServer::start().await;
    let (_ns, key) = common::signup(&server.base_url, "Hidx AC4").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let result = client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("host.quickstart");
    let quickstart = common::extract_structured(&result);
    let plan = mcphost::plans::PlanCatalog::default_catalog().get("free").expect("free plan").clone();
    assert_eq!(quickstart["limits"]["plan"]["index_max"], json!(plan.index_max), "{quickstart}");
    assert_eq!(quickstart["limits"]["plan"]["index_max"], json!(3));
    assert_eq!(quickstart["limits"]["plan"]["index_chunks_max"], json!(plan.index_chunks_max));
}
