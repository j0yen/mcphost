//! PRD-mcphost-dry-run-inside-savepoint-for-transactional-verbs
//! AC5 / R4 / R6 -- every non-`Publish` verb in `write_verbs::DRY_RUN_VERBS`
//! is run with `dry_run: true` through real dispatch on a fresh, seeded
//! tenant: no error, `would` present, `_meta.dry_run == true`, and the
//! read-back through a read verb is unchanged. The fixture table is keyed by
//! verb name and its key set must equal the registry's. Fixtures seed
//! through dispatch; a row marked `seed: db` seeds through direct `Db` calls
//! only because its resource needs an external party. Plus the two
//! `host.tool_test` paths that share the dry-run connection.
//! contract: reads registry write_verbs::DRY_RUN_VERBS

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::write_verbs::{DRY_RUN_VERBS, DryRun, Kind, lookup};
use serde_json::{Value, json};
use std::collections::BTreeSet;

thread_local! {
    static OTHER_NS: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

struct Fx {
    verb: &'static str,
    /// `seed: db` -- the target resource needs an external party, so the
    /// seeding (never the read-back) goes through direct `Db` calls.
    seed_db: bool,
    seed: Vec<(&'static str, Value)>,
    call: Value,
    reads: Vec<(&'static str, Value)>,
}

fn fx(verb: &'static str, seed: Vec<(&'static str, Value)>, call: Value, reads: Vec<(&'static str, Value)>) -> Fx {
    Fx { verb, seed_db: false, seed, call, reads }
}

fn fx_db(verb: &'static str, seed: Vec<(&'static str, Value)>, call: Value, reads: Vec<(&'static str, Value)>) -> Fx {
    Fx { verb, seed_db: true, seed, call, reads }
}

fn tbl() -> Vec<(&'static str, Value)> {
    vec![("host.table.create", json!({"name": "t1", "columns": {"a": "text"}}))]
}
fn tbl_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("host.table.create", json!({"name": "t1", "columns": {"a": "text"}})),
        ("host.table.append", json!({"table": "t1", "rows": [{"a": "x"}, {"a": "y"}]})),
    ]
}
fn st_tbl() -> Vec<(&'static str, Value)> {
    vec![("host.state.table_create", json!({"name": "st1", "schema": {"a": "text"}}))]
}
fn st_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("host.state.table_create", json!({"name": "st1", "schema": {"a": "text"}})),
        ("host.state.insert", json!({"table": "st1", "rows": [{"a": "x"}, {"a": "y"}, {"a": "z"}]})),
    ]
}
fn st_reads() -> Vec<(&'static str, Value)> {
    vec![("host.state.list", json!({})), ("host.state.query", json!({"table": "st1"}))]
}
fn docs2() -> Vec<(&'static str, Value)> {
    vec![
        ("host.docs.put", json!({"name": "d1", "content": "one"})),
        ("host.docs.put", json!({"name": "d1", "content": "two"})),
    ]
}
fn publish() -> (&'static str, Value) {
    ("host.tool.publish", json!({"name": "pinger", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
}
fn trig() -> Vec<(&'static str, Value)> {
    vec![publish(), ("host.trigger.set", json!({"name": "tg1", "tool": "pinger", "schedule": "0 0 1 1 *"}))]
}

fn fixtures() -> Vec<Fx> {
    let d = |v: Value| {
        let mut v = v;
        v["dry_run"] = json!(true);
        v
    };
    vec![
        fx("host.table.create", vec![], d(json!({"name": "t1", "columns": {"a": "text"}})), vec![("host.table.list", json!({}))]),
        fx("host.table.append", tbl(), d(json!({"table": "t1", "rows": [{"a": "x"}]})), vec![("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM t1"}))]),
        fx("host.table.drop", tbl_rows(), d(json!({"name": "t1"})), vec![("host.table.list", json!({})), ("host.table.query", json!({"sql": "SELECT COUNT(*) AS n FROM t1"}))]),
        fx("host.table.handle_drop", { let mut v = tbl_rows(); v.push(("host.table.query", json!({"sql": "SELECT * FROM t1", "handle": true}))); v }, d(json!({"handle": "{last.handle}"})), vec![("host.table.handles", json!({})), ("host.table.list", json!({}))]),
        fx("host.table.model_set", tbl(), d(json!({"table": "t1", "key": "description", "value": "x"})), vec![("host.table.models", json!({}))]),
        fx("host.state.set", vec![], d(json!({"key": "k1", "value": "v"})), vec![("host.state.get", json!({"key": "k1"})), ("host.state.list", json!({}))]),
        fx("host.state.insert", st_tbl(), d(json!({"table": "st1", "rows": [{"a": "x"}]})), st_reads()),
        fx("host.state.delete", vec![("host.state.set", json!({"key": "k1", "value": "v"}))], d(json!({"key": "k1"})), vec![("host.state.get", json!({"key": "k1"})), ("host.state.list", json!({}))]),
        fx("host.state.delete_rows", st_rows(), d(json!({"table": "st1"})), st_reads()),
        fx("host.state.table_create", vec![], d(json!({"name": "st1", "schema": {"a": "text"}})), vec![("host.state.list", json!({}))]),
        fx("host.state.table_drop", st_rows(), d(json!({"name": "st1"})), st_reads()),
        fx("host.docs.put", vec![], d(json!({"name": "d1", "content": "one"})), vec![("host.docs.list", json!({}))]),
        fx("host.docs.put_many", vec![], d(json!({"documents": [{"name": "d1", "content": "one"}, {"name": "d2", "content": "two"}]})), vec![("host.docs.list", json!({}))]),
        fx("host.docs.delete", vec![("host.docs.put", json!({"name": "d1", "content": "one"}))], d(json!({"name": "d1"})), vec![("host.docs.list", json!({})), ("host.docs.get", json!({"name": "d1"}))]),
        fx("host.docs.purge", docs2(), d(json!({"name": "d1", "older_than_versions": 1})), vec![("host.docs.list", json!({})), ("host.docs.get", json!({"name": "d1"}))]),
        fx("host.index.create", vec![], d(json!({"name": "ix1", "sources": [{"kind": "doc", "prefix": "d"}]})), vec![("host.index.list", json!({}))]),
        fx("host.index.delete", vec![("host.index.create", json!({"name": "ix1", "sources": [{"kind": "doc", "prefix": "d"}]}))], d(json!({"name": "ix1"})), vec![("host.index.list", json!({}))]),
        fx_db("host.enduser.purge", vec![], d(json!({"subject": "eu-1"})), vec![("host.enduser.get", json!({"subject": "eu-1"}))]),
        fx_db("host.enduser.revoke", vec![], d(json!({"subject": "eu-1"})), vec![("host.enduser.get", json!({"subject": "eu-1"}))]),
        fx_db("host.enduser.unrevoke", vec![], d(json!({"subject": "eu-1"})), vec![("host.enduser.get", json!({"subject": "eu-1"}))]),
        fx_db("host.oauth.revoke_all", vec![], d(json!({})), vec![("host.oauth.grants", json!({}))]),
        fx_db("host.oauth.grant_revoke", vec![], d(json!({"id": 1})), vec![("host.oauth.grants", json!({}))]),
        fx("host.oauth.issuer_set", vec![], d(json!({"issuer": "https://issuer.example.com", "jwks_url": "https://issuer.example.com/jwks.json"})), vec![("host.oauth.issuers", json!({}))]),
        fx("host.oauth.issuer_remove", vec![("host.oauth.issuer_set", json!({"issuer": "https://issuer.example.com", "jwks_url": "https://issuer.example.com/jwks.json"}))], d(json!({"issuer": "https://issuer.example.com"})), vec![("host.oauth.issuers", json!({}))]),
        fx("host.oauth.scope_set", vec![], d(json!({"name": "notes:read", "description": "Read notes"})), vec![("host.oauth.scopes", json!({}))]),
        fx("host.oauth.policy_set", vec![], d(json!({"clients": "allowlist", "allowlist": ["c1"]})), vec![("host.oauth.policy", json!({}))]),
        fx("host.oauth.trusted_issuer_set", vec![], d(json!({"issuer": "https://ti.example.com", "jwks_url": "https://ti.example.com/jwks.json", "client_id": "c1"})), vec![("host.oauth.trusted_issuers", json!({}))]),
        fx("host.oauth.trusted_issuer_remove", vec![("host.oauth.trusted_issuer_set", json!({"issuer": "https://ti.example.com", "jwks_url": "https://ti.example.com/jwks.json", "client_id": "c1"}))], d(json!({"issuer": "https://ti.example.com"})), vec![("host.oauth.trusted_issuers", json!({}))]),
        fx("host.runs.purge", vec![], d(json!({"before_unix": 4_000_000_000_i64})), vec![("host.runs.list", json!({}))]),
        fx("host.tool.remove", vec![publish()], d(json!({"name": "pinger"})), vec![("host.tool.list", json!({}))]),
        fx("host.trigger.set", vec![publish()], d(json!({"name": "tg1", "tool": "pinger", "schedule": "0 0 1 1 *"})), vec![("host.trigger.list", json!({}))]),
        fx("host.trigger.pause", trig(), d(json!({"name": "tg1"})), vec![("host.trigger.list", json!({}))]),
        fx("host.trigger.resume", trig(), d(json!({"name": "tg1"})), vec![("host.trigger.list", json!({}))]),
        fx("host.trigger.remove", trig(), d(json!({"name": "tg1"})), vec![("host.trigger.list", json!({}))]),
        fx("host.group.create", vec![], d(json!({"name": "g1"})), vec![("host.group.list", json!({}))]),
        fx("host.group.remove", vec![("host.group.create", json!({"name": "g1"}))], d(json!({"name": "g1", "namespace": "{ns}"})), vec![("host.group.list", json!({}))]),
        fx("host.invite.create", vec![], d(json!({})), vec![("host.invite.list", json!({}))]),
        fx("host.ask.create", vec![], d(json!({"question": "Ship it?", "options": ["yes", "no"]})), vec![("host.ask.list", json!({}))]),
        fx("host.invite.revoke", vec![("host.invite.create", json!({}))], d(json!({"code": "{last.code}"})), vec![("host.invite.list", json!({}))]),
        fx("host.secret.set", vec![], d(json!({"name": "s1", "value": "v"})), vec![("host.secret.list", json!({}))]),
        fx("host.policy.set", tbl(), d(json!({"target": {"table": "t1"}, "rule": [{"column_or_attr": "a", "op": "eq", "value": {"literal": "x"}}]})), vec![("host.policy.list", json!({}))]),
        fx("host.policy.attrs_set", vec![], d(json!({"subject": "eu-1", "attrs": {"role": "x"}})), vec![("host.policy.list", json!({}))]),
        fx("host.agent.profile_set", vec![], d(json!({"description": "hello"})), vec![("host.agent.whoami", json!({}))]),
        fx("host.share.caller_limit_remove", vec![publish(), ("host.tool.share", json!({"name": "pinger", "visibility": "public"})), ("host.share.caller_limit", json!({"tool": "pinger", "caller_tenant": "{other_ns}", "calls_per_day": 3}))], d(json!({"tool": "pinger", "caller_tenant": "{other_ns}"})), vec![("host.tool.list", json!({})), ("host.tool.list_shared", json!({}))]),
        fx("host.org.digest.set", vec![("host.org.create", json!({"name": "acme"}))], d(json!({"enabled": false})), vec![("host.org.usage", json!({}))]),
    ]
}

/// Output keys the read tools compute from the current time on every call
/// (`now - <stored timestamp>`): two reads straddling a second boundary differ
/// in them with no state change, so the read-back compare masks them. Stored
/// timestamps (`created_at`, `updated_at`, ...) are stable between reads and
/// stay compared. Each key's defining file is in `CLOCK_DERIVED_KEY_SRC`.
const CLOCK_DERIVED_READ_KEYS: &[&str] = &["lag_seconds"];

/// key -> the `src/` file whose code emits it. `clock_derived_keys_are_current`
/// reads each file, so a renamed or removed key fails here instead of leaving
/// a dead entry in the mask.
// lag_seconds: src/index.rs:579 (`host.index.list`, `now - created_at` while a source is pending)
const CLOCK_DERIVED_KEY_SRC: &[(&str, &str)] = &[("lag_seconds", "src/index.rs")];

const CLOCK_MARKER: &str = "<clock>";

/// `v` with the value of every clock-derived key, at any depth, replaced by a
/// fixed marker.
fn mask_clock(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| {
                    let x = if CLOCK_DERIVED_READ_KEYS.contains(&k.as_str()) { json!(CLOCK_MARKER) } else { mask_clock(x) };
                    (k.clone(), x)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(mask_clock).collect()),
        o => o.clone(),
    }
}

/// The one read-back compare path: equal once clock-derived fields are masked.
fn same_readback(before: &Result<Value, String>, after: &Result<Value, String>) -> bool {
    let mask = |r: &Result<Value, String>| r.as_ref().map(mask_clock).map_err(String::clone);
    mask(before) == mask(after)
}

fn same_snapshot(before: &[Value], after: &[Value]) -> bool {
    before.len() == after.len() && before.iter().zip(after).all(|(b, a)| same_readback(&Ok(b.clone()), &Ok(a.clone())))
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    s.chars().take(300).collect()
}

fn subst(v: &Value, ns: &str, last: &Value) -> Value {
    match v {
        Value::String(s) if s == "{other_ns}" => json!(OTHER_NS.with(|c| c.borrow().clone())),
        Value::String(s) if s == "{ns}" => json!(ns),
        Value::String(s) if s.starts_with("{last.") => last[&s[6..s.len() - 1]].clone(),
        Value::Array(a) => Value::Array(a.iter().map(|x| subst(x, ns, last)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), subst(x, ns, last))).collect()),
        o => o.clone(),
    }
}

async fn seed_db(server: &TestServer, tenant_id: i64, verb: &str) {
    let db = &server.state.db;
    match verb {
        "host.enduser.purge" => {
            db.insert_end_user_for_test(tenant_id, "eu-1".into(), 100, Some(100), None).await.expect("end user");
        }
        "host.enduser.revoke" => {
            db.insert_end_user_for_test(tenant_id, "eu-1".into(), 100, None, None).await.expect("end user");
        }
        "host.enduser.unrevoke" => {
            db.insert_end_user_for_test(tenant_id, "eu-1".into(), 100, Some(100), None).await.expect("end user");
        }
        "host.oauth.revoke_all" | "host.oauth.grant_revoke" => {
            db.create_oauth_grant(tenant_id, "c1".into(), None, "dcr".into(), "https://r".into(), "mcp".into(), None)
                .await
                .expect("grant");
        }
        other => panic!("no db seed for {other}"),
    }
}

#[tokio::test]
async fn dryspt_ac05() {
    // The registry set, by symbol: never re-listed here.
    let registry: BTreeSet<&str> = DRY_RUN_VERBS
        .iter()
        .copied()
        .filter(|v| lookup(v).map(|r| r.kind) != Some(Kind::Publish))
        .collect();
    assert!(DRY_RUN_VERBS.iter().all(|v| matches!(lookup(v).unwrap().dry_run, DryRun::Supported)));
    let table = fixtures();
    let keys: BTreeSet<&str> = table.iter().map(|f| f.verb).collect();
    assert_eq!(table.len(), keys.len(), "duplicate fixture rows");
    let missing: Vec<_> = registry.difference(&keys).collect();
    let extra: Vec<_> = keys.difference(&registry).collect();
    assert!(missing.is_empty() && extra.is_empty(), "fixture keys != registry: no fixture for {missing:?}; fixture for a verb not in the registry {extra:?}");

    let mut failures: Vec<String> = Vec::new();
    for f in &table {
        let server = TestServer::start().await;
        let (ns, key) = signup(&server.base_url, "Dryspt AC5").await;
        let client = McpClient::with_bearer(&server.base_url, &key);
        let (other_ns, _) = signup(&server.base_url, "Dryspt AC5 other").await;
        OTHER_NS.with(|c| *c.borrow_mut() = other_ns);
        let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("tenant lookup").expect("tenant");

        let mut last = Value::Null;
        if f.seed_db {
            seed_db(&server, tenant.id, f.verb).await;
        }
        let mut seed_err = None;
        for (tool, args) in &f.seed {
            match client.tools_call(tool, subst(args, &ns, &last)).await {
                Ok(r) => last = extract_structured(&r),
                Err(e) => {
                    seed_err = Some(format!("seed {tool}: {} {}", e.code, e.message));
                    break;
                }
            }
        }
        if let Some(e) = seed_err {
            failures.push(format!("{}: {e}", f.verb));
            continue;
        }
        let mut before = Vec::new();
        for (tool, args) in &f.reads {
            before.push(client.tools_call(tool, subst(args, &ns, &last)).await.map(|r| extract_structured(&r)).map_err(|e| format!("{} {}", e.code, e.message)));
        }
        let nested_before = mcphost::txn::counters::nested_calls();
        let res = client.tools_call(f.verb, subst(&f.call, &ns, &last)).await;
        let nested = mcphost::txn::counters::nested_calls() - nested_before;
        let outcome = match &res {
            Ok(r) => {
                let s = extract_structured(r);
                if s.get("would").is_none() {
                    format!("no `would`: {}", short(&s))
                } else if s["_meta"]["dry_run"] != json!(true) {
                    format!("_meta.dry_run != true: {}", short(&s))
                } else {
                    "would".to_string()
                }
            }
            Err(e) => format!("ERROR {} {} {}", e.code, e.message, e.error_code.clone().unwrap_or_default()),
        };
        println!("dryspt-sweep verb={} outcome={} nested_with_tx={}", f.verb, outcome.chars().take(60).collect::<String>(), nested);
        if outcome != "would" {
            failures.push(format!("{}: {outcome}", f.verb));
            continue;
        }
        for (i, (tool, args)) in f.reads.iter().enumerate() {
            let after = client.tools_call(tool, subst(args, &ns, &last)).await.map(|r| extract_structured(&r)).map_err(|e| format!("{} {}", e.code, e.message));
            if !same_readback(&before[i], &after) {
                failures.push(format!("{}: read-back {tool} changed: before {} after {}", f.verb, short(&json!(format!("{:?}", before[i]))), short(&json!(format!("{:?}", after)))));
            }
        }
    }
    assert!(failures.is_empty(), "dry-run sweep failures:\n{}", failures.join("\n"));

    tool_test_forwarded_verb().await;
    tool_test_python_bridge().await;
}

/// Seeds `st1` with three rows; returns the read-back snapshot.
async fn seed_and_snapshot(client: &McpClient) -> Vec<Value> {
    for (tool, args) in st_rows() {
        client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("seed {tool}: {} {}", e.code, e.message));
    }
    snapshot(client).await
}

async fn snapshot(client: &McpClient) -> Vec<Value> {
    let mut out = Vec::new();
    for (tool, args) in st_reads() {
        out.push(extract_structured(&client.tools_call(tool, args).await.expect("read-back")));
    }
    out
}

/// Row 1 (`src/handler.rs` `test_forward_host_verb`): `host.tool_test` naming
/// a host verb dry-runs it on the savepoint connection.
async fn tool_test_forwarded_verb() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Dryspt AC5 forwarded").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let before = seed_and_snapshot(&client).await;
    let r = client
        .tools_call("host.tool_test", json!({"name": "host.state.table_drop", "args": {"name": "st1"}}))
        .await
        .unwrap_or_else(|e| panic!("forwarded host.tool_test errored: {} {} {:?}", e.code, e.message, e.error_code));
    println!("dryspt-sweep verb=host.tool_test(host.state.table_drop) outcome=ok result={}", short(&extract_structured(&r)));
    let after = snapshot(&client).await;
    assert!(same_snapshot(&before, &after), "the table and its rows remain: before {before:?} after {after:?}");
}

/// Row 2 (`src/handler.rs` published-tool path): a python tool whose body
/// calls the state bridge op `table_drop`, run through `host.tool_test`.
async fn tool_test_python_bridge() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(common::python_kind_registry(&envs.0)).await;
    let (_ns, key) = signup(&server.base_url, "Dryspt AC5 python").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let before = seed_and_snapshot(&client).await;
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "dropper", "kind": "python", "spec": {"source": "from mcphost import state\ndef main(args):\n    return state.table_drop(\"st1\")\n"}}),
        )
        .await
        .expect("publish dropper");
    let r = client
        .tools_call("host.tool_test", json!({"name": "dropper", "args": {}}))
        .await
        .unwrap_or_else(|e| panic!("python host.tool_test errored: {} {} {:?}", e.code, e.message, e.error_code));
    println!("dryspt-sweep verb=host.tool_test(python table_drop) outcome=ok result={}", short(&extract_structured(&r)));
    let after = snapshot(&client).await;
    assert!(same_snapshot(&before, &after), "the table and its rows remain: before {before:?} after {after:?}");
}

/// No server: every masked key is still emitted by the `src/` file its pair
/// names, the pairs cover exactly the mask list, and stored timestamps are
/// never masked.
#[test]
fn clock_derived_keys_are_current() {
    let listed: BTreeSet<&str> = CLOCK_DERIVED_READ_KEYS.iter().copied().collect();
    let paired: BTreeSet<&str> = CLOCK_DERIVED_KEY_SRC.iter().map(|(k, _)| *k).collect();
    assert_eq!(listed, paired, "CLOCK_DERIVED_READ_KEYS and CLOCK_DERIVED_KEY_SRC name different keys");
    for forbidden in ["created_at", "updated_at"] {
        assert!(!listed.contains(forbidden), "{forbidden} is a stored timestamp, stable between reads: it must stay compared");
    }
    for (key, file) in CLOCK_DERIVED_KEY_SRC {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        assert!(src.contains(&format!("\"{key}\"")), "{file} no longer emits \"{key}\": drop it from CLOCK_DERIVED_READ_KEYS or fix the pair");
    }
    assert_eq!(mask_clock(&json!({"a": [{"lag_seconds": 3, "created_at": 1}]})), json!({"a": [{"lag_seconds": CLOCK_MARKER, "created_at": 1}]}));
}
