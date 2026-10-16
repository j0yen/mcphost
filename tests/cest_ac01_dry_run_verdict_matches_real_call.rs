//! PRD-mcphost-call-cost-estimate
//! AC1 -- Given 200 fixtures of (tenant plan, used calls, tool, args, budget)
//! spanning every `PREFLIGHT_CHECKS` row, When each is run as `dry_run: true`
//! and then as a real call, Then `fits` equals the real call's admission (ok
//! vs refused) in 200/200 and every non-null `refusal` equals the real
//! error's `code`, `message` and `data` byte for byte.
//!
//! `data.request_id` is the one field excluded from the byte comparison: it
//! is the per-response correlation id `AppError::into_error_data_at` mints
//! fresh for every error, so two responses can never share it.

use crate::common;
use crate::slotkind;

use slotkind::SlotKind;
use common::{McpClient, TestServer, extract_structured, record_ok_calls, signup, signup_and_make_pro};
use mcphost::preflight::PREFLIGHT_CHECKS;
use serde_json::{Value, json};

struct Fixture<'a> {
    client: &'a McpClient,
    name: String,
    args: Value,
    budget: Option<Value>,
    /// Hold every slot while this fixture runs (the `capacity` row).
    full: bool,
}

fn strip_request_id(mut data: Value) -> Value {
    if let Some(obj) = data.as_object_mut() {
        obj.remove("request_id");
        // call_id is per-call by contract (mcphost-call-trace-replay); it can never match across two calls
        obj.remove("call_id");
    }
    data
}

async fn used_today(server: &TestServer, tenant_id: i64) -> i64 {
    let midnight = mcphost::state::utc_midnight_unix(mcphost::state::now_unix());
    server.state.db.count_calls_since(tenant_id, midnight, true).await.expect("count")
}

async fn tenant_id(server: &TestServer, ns: &str) -> i64 {
    server.state.db.find_tenant_by_namespace(ns.to_string()).await.expect("db").expect("tenant").id
}

async fn publish_slot(client: &McpClient, name: &str) {
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": name, "kind": "slot", "spec": {"schema": {
                "type": "object",
                "properties": {"n": {"type": "integer"}},
                "required": ["n"],
            }}}),
        )
        .await
        .expect("publish slot tool");
}

// Not `#[tokio::test]`: this drives 200 real `host.tool_call`s, including a
// forwarded host verb (`host.usage`) whose nested dispatch needs ~1.9 MiB of
// stack in an unoptimized build -- within a few percent of the libtest
// thread's 2 MiB, and over it once an unrelated change grew a handler frame
// (CI SIGABRT "has overflowed its stack" in `suite_core_07`, three branches
// rebased onto 5cfa2fad). `block_on_big_stack` runs the same body on a
// 64 MiB-stack thread, so the test no longer depends on the default.
#[test]
fn dry_run_verdict_equals_real_admission_in_200_of_200() {
    common::block_on_big_stack(run_200_fixtures);
}

async fn run_200_fixtures() {
    let slot = SlotKind::new(4);
    let server = TestServer::start_with_kinds(slot.registry()).await;

    let (ns_free, key_free) = signup(&server.base_url, "Cest Free").await;
    let (_ns_pro, key_pro, pro_id) = signup_and_make_pro(&server, "Cest Pro", "cus_cest_ac01").await;
    let free = McpClient::with_bearer(&server.base_url, &key_free);
    let pro = McpClient::with_bearer(&server.base_url, &key_pro);
    let free_id = tenant_id(&server, &ns_free).await;
    publish_slot(&free, "tt").await;
    publish_slot(&pro, "tt").await;

    // Owner/caller pair for the `caller_limit` row: `owner` shares "t" and
    // caps `caller` at 3 calls a day.
    let (ns_owner, key_owner) = signup(&server.base_url, "Cest Owner").await;
    let (ns_caller, key_caller) = signup(&server.base_url, "Cest Caller").await;
    let owner = McpClient::with_bearer(&server.base_url, &key_owner);
    let caller = McpClient::with_bearer(&server.base_url, &key_caller);
    publish_slot(&owner, "tt").await;
    owner
        .tools_call("host.tool_share", json!({"name": "tt", "visibility": "public"}))
        .await
        .expect("share");
    owner
        .tools_call(
            "host.share.caller_limit",
            json!({"tool": "tt", "caller_tenant": ns_caller, "calls_per_day": 3}),
        )
        .await
        .expect("caller limit");

    let ceilings = server.state.plans.get("free").expect("free plan").budget_defaults();
    let over = |field: &str, ceiling: Option<i64>| json!({ field: ceiling.expect("ceiling") + 1 });
    let budgets: Vec<Option<Value>> = vec![
        None,
        Some(json!({"max_child_calls": 1})),
        Some(over("max_child_calls", ceilings.max_child_calls)),
        Some(over("max_est_tokens", ceilings.max_est_tokens)),
        Some(over("max_tool_latency_ms", ceilings.max_tool_latency_ms)),
        Some(over("max_wall_ms", ceilings.max_wall_ms)),
        Some(json!({"max_wall_ms": "soon"})),
    ];
    let args_set = [json!({"n": 1}), json!({"n": "one"}), json!({})];

    let mut fixtures: Vec<(Option<i64>, Fixture<'_>)> = Vec::new();
    // (used-calls level the free tenant is raised to first, fixture)
    for level in [0, 250, 498, 499, 500, 503] {
        for args in &args_set {
            for budget in &budgets {
                fixtures.push((
                    Some(level),
                    Fixture { client: &free, name: "tt".into(), args: args.clone(), budget: budget.clone(), full: false },
                ));
            }
        }
    }
    // The pro tenant (a different plan, so different ceilings) at zero used.
    for args in &args_set {
        for budget in &budgets {
            fixtures.push((
                None,
                Fixture { client: &pro, name: "tt".into(), args: args.clone(), budget: budget.clone(), full: false },
            ));
        }
    }
    // `capacity`: every slot held while the call is asked.
    for client in [&pro, &caller] {
        for args in &args_set {
            for budget in [None, Some(json!({"max_child_calls": 1}))] {
                let name = if std::ptr::eq(client, &caller) { format!("{ns_owner}.tt") } else { "tt".into() };
                fixtures.push((
                    None,
                    Fixture { client, name, args: args.clone(), budget, full: true },
                ));
            }
        }
    }
    // `caller_limit`: the caller works through its 3-call allowance and past it.
    for i in 0..8 {
        let args = if i == 5 { json!({"n": "x"}) } else { json!({"n": i}) };
        fixtures.push((
            None,
            Fixture { client: &caller, name: format!("{ns_owner}.tt"), args, budget: None, full: false },
        ));
    }
    // Resolution misses and a forwardable host verb.
    for name in ["nope", "no_such_ns.tt", "host.usage", "billing.plans", "host.no_such_verb"] {
        fixtures.push((
            None,
            Fixture { client: &pro, name: name.into(), args: json!({}), budget: None, full: false },
        ));
    }
    // Pad (or trim) to exactly 200 with plain pro-tenant calls.
    while fixtures.len() < 200 {
        let i = fixtures.len() as i64;
        fixtures.push((
            None,
            Fixture { client: &pro, name: "tt".into(), args: json!({"n": i}), budget: None, full: false },
        ));
    }
    fixtures.truncate(200);
    assert_eq!(fixtures.len(), 200);

    let mut refusal_codes = std::collections::BTreeSet::new();
    let mut fits_true = 0;
    for (idx, (level, fx)) in fixtures.iter().enumerate() {
        if let Some(level) = level {
            let have = used_today(&server, free_id).await;
            if have < *level {
                record_ok_calls(&server, free_id, "tt", (*level - have) as usize).await;
            }
        }
        let held = if fx.full {
            Some(slot.semaphore().try_acquire_many_owned(4).expect("hold every slot"))
        } else {
            None
        };

        let mut dry_args = json!({"name": fx.name, "args": fx.args, "dry_run": true});
        let mut real_args = json!({"name": fx.name, "args": fx.args});
        if let Some(b) = &fx.budget {
            dry_args["budget"] = b.clone();
            real_args["budget"] = b.clone();
        }
        let calls_before = used_today(&server, free_id).await + used_today(&server, pro_id).await;
        let dry = extract_structured(&fx.client.tools_call("host.tool_call", dry_args).await.expect("dry run answers"));
        let calls_after_dry = used_today(&server, free_id).await + used_today(&server, pro_id).await;
        assert_eq!(calls_before, calls_after_dry, "fixture {idx}: a dry run wrote an ok call row");
        assert_eq!(dry["dry_run"], json!(true), "fixture {idx}: {dry}");

        let real = fx.client.tools_call("host.tool_call", real_args).await;
        drop(held);

        match real {
            Ok(_) => {
                assert_eq!(dry["fits"], json!(true), "fixture {idx} ({}): real call was admitted, dry run said {dry}", fx.name);
                assert_eq!(dry["refusal"], Value::Null, "fixture {idx}: {dry}");
                fits_true += 1;
            }
            Err(e) => {
                assert_eq!(dry["fits"], json!(false), "fixture {idx} ({}): real call refused {e:?}, dry run said {dry}", fx.name);
                let refusal = &dry["refusal"];
                let code = e.error_code.clone().expect("real refusal has a code");
                assert_eq!(refusal["code"], json!(code), "fixture {idx}: code");
                assert_eq!(refusal["message"], json!(e.message), "fixture {idx}: message");
                assert_eq!(
                    strip_request_id(refusal["data"].clone()),
                    strip_request_id(e.data.clone()),
                    "fixture {idx}: data"
                );
                refusal_codes.insert(code);
            }
        }
    }

    // Every PREFLIGHT_CHECKS row was exercised by at least one refusal.
    for (row, code) in [
        ("budget_ceiling", "budget_ceiling_exceeded"),
        ("args_schema", "args_invalid"),
        ("calls_per_day", "quota_exceeded"),
        ("caller_limit", "quota_caller"),
        ("capacity", "capacity"),
    ] {
        assert!(PREFLIGHT_CHECKS.iter().any(|c| c.id == row), "{row} is a PREFLIGHT_CHECKS row");
        assert!(refusal_codes.contains(code), "no fixture refused on {row} ({code}); saw {refusal_codes:?}");
    }
    assert!(fits_true > 0, "some fixtures must be admitted");
}
