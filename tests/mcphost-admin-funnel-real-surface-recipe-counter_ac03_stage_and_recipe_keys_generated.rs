//! PRD-mcphost-admin-funnel-real-surface-recipe-counter
//! AC3 (P0) — Given the consts `SURFACE_STAGE_KEYS` and the `recipe_names()`
//! set, both generated into `src/api_contract.rs`, When `admin.funnel`,
//! `mcphost funnel --json` and the tool description are built for one
//! database, Then each `by_surface` row has exactly the const keys,
//! `recipe_attempts_7d` has exactly the recipe names, both values equal the
//! CLI's, a drift test fails when either set changes alone, and the API
//! contract lists both sets verbatim.

use crate::common;
use mcphost::api_contract::{SURFACE_STAGE_KEYS, admin_funnel_contract, dump_contract};
use mcphost::auth::{generate_key, generate_namespace, hash_key};
use mcphost::kinds::KindRegistry;
use mcphost::kinds::aliases::recipe_names;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

#[tokio::test]
async fn admin_funnel_cli_description_and_contract_share_one_key_set_each() {
    let (state, dir) = common::bare_app_state().await;
    let now = mcphost::state::now_unix();
    let t = state
        .db
        .create_tenant_attributed_with_source(
            "A".to_string(),
            generate_namespace(),
            hash_key(&generate_key()),
            None,
            Some("external".to_string()),
            Some("cursor".to_string()),
            None,
            "external".to_string(),
            None,
            None,
            "human".to_string(),
        )
        .await
        .expect("create tenant");
    state.db.set_tenant_stamp_for_test(t.id, "created_unix", now - 86_400).await.expect("backdate");

    let admin = mcphost::admin::funnel(&state, &json!({"days": 7})).await.expect("admin.funnel");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["funnel", "--json", "--days", "7"])
        .env("MCPHOST_DATA_DIR", &dir.0)
        .output()
        .expect("spawn mcphost funnel");
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    let cli: Value = serde_json::from_slice(&output.stdout).expect("--json parses");

    let stage_keys: BTreeSet<String> = SURFACE_STAGE_KEYS.iter().map(|k| k.to_string()).collect();
    let recipes: BTreeSet<String> = recipe_names().into_iter().map(String::from).collect();
    for (label, body) in [("admin", &admin), ("cli", &cli)] {
        let rows = body["by_surface"].as_object().expect("by_surface");
        assert!(rows.contains_key("cursor"), "{label}: {body}");
        for (surface, row) in rows {
            assert_eq!(keys(row), stage_keys, "{label}: row {surface} keys != SURFACE_STAGE_KEYS");
        }
        assert_eq!(keys(&body["recipe_attempts_7d"]), recipes, "{label}: {body}");
        assert!(body["recipe_attempts_synthetic_7d"].is_i64(), "{label}: {body}");
    }
    assert_eq!(admin["by_surface"], cli["by_surface"]);
    assert_eq!(admin["recipe_attempts_7d"], cli["recipe_attempts_7d"]);

    // The tool description names both sets verbatim.
    let desc = mcphost::handler::admin_tool_descriptors()
        .into_iter()
        .find(|t| t.name == "admin.funnel")
        .and_then(|t| t.description.map(|d| d.to_string()))
        .expect("admin.funnel description");
    assert!(desc.contains(&SURFACE_STAGE_KEYS.join(", ")), "{desc}");
    assert!(desc.contains(&recipe_names().join(", ")), "{desc}");

    // The contract lists both sets verbatim, from the same consts.
    let contract = dump_contract(&KindRegistry::with_builtin());
    assert_eq!(contract["admin_funnel"], admin_funnel_contract());
    assert_eq!(contract["admin_funnel"]["surface_stage_keys"], json!(SURFACE_STAGE_KEYS));
    assert_eq!(contract["admin_funnel"]["recipe_attempts_7d_keys"], json!(recipe_names()));
    // Drift: the committed contract must carry the live sets, so changing
    // either const alone (without regenerating) fails here.
    let committed: Value = serde_json::from_slice(&std::fs::read(mcphost::api_contract::DEFAULT_CONTRACT_PATH).expect("read"))
        .expect("parse committed contract");
    assert_eq!(committed["admin_funnel"], admin_funnel_contract(), "run `mcphost contract dump`");
}
