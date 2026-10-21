//! AC3 — Given `TOOLS_LIST_OPS` and `sublis_ac03_drift` with caller, target and
//! bystander streams, When every row runs and a debug test sets `CURRENT_VERB`
//! to `test.unlisted` and inserts a tool row via the db handle, Then each row
//! reaches each `affected` tenant once, no other; the unlisted write trips the
//! debug write-check naming its verb and `TOOLS_LIST_OPS`; share/unshare
//! deliver per their row's `affected`; and `write_hint` and prompts tracking
//! match `TOOLS_LIST_OPS` on every row.

use crate::common;
use crate::promptkind;
use crate::timp;
use promptkind::PromptKind;
use common::{ListenStream, McpClient, TestServer, extract_structured, signup};
use mcphost::listen::{Affected, Effect, TOOLS_LIST_OPS, ToolsListOp};
use serde_json::{Value, json};
use std::time::Duration;

const CALLER: usize = 0;
const TARGET: usize = 1;
const BYSTANDER: usize = 2;
const EMPTY: usize = 3;
const WINDOW: Duration = Duration::from_millis(1500);

/// Rows whose verb is driven by a feature test of its own (they need a drafter,
/// a probe target, a mock upstream or a webhook plan); each file runs the verb
/// under the same debug write-check, so an unlisted one would fail it.
const DRIVEN_ELSEWHERE: &[(&str, &str)] = &[
    ("host.tool_from_description", "tests/tfd_ac03_retry_then_publish_and_share.rs"),
    ("host.uptime.create", "tests/mcphost_uptime_probes_ac02_checks_two_rows_per_target.rs"),
    ("host.webhook_inbox.create", "tests/mcphost_webhook_inbox_ac01_set_returns_url_and_secret_get_omits_secret.rs"),
    ("host.secret.leak_test", "tests/snp_ac04_leak_test_proves_both_legs_blocked.rs"),
];

struct Actor {
    ns: String,
    client: McpClient,
    stream: ListenStream,
}

/// Which actor each `Affected` kind resolves to in this run.
struct Ctx {
    caller: usize,
    target: Option<usize>,
    members: Vec<usize>,
}

struct Fx {
    server: TestServer,
    actors: Vec<Actor>,
    note_key: Option<String>,
    /// Every actor's tools/list names, taken once setup has settled.
    before: Vec<Vec<String>>,
}

fn echo_spec(variant: &str) -> Value {
    json!({"schema": {"type": "object", "properties": {variant: {"type": "string"}}}})
}

async fn call(client: &McpClient, tool: &str, args: Value) -> Value {
    extract_structured(&client.tools_call(tool, args).await.unwrap_or_else(|e| panic!("{tool} failed: {} {}", e.code, e.message)))
}

async fn try_call(client: &McpClient, tool: &str, args: Value) {
    let _ = client.tools_call(tool, args).await;
}

/// Every notification method that reaches `stream` within `window`.
async fn gather(stream: &mut ListenStream, window: Duration) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + window;
    let mut methods = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match stream.next_method(left).await {
            Some((method, _)) => methods.push(method),
            None => return methods,
        }
    }
}

async fn gather_all(actors: &mut [Actor], window: Duration) -> Vec<Vec<String>> {
    futures_util::future::join_all(actors.iter_mut().map(|a| gather(&mut a.stream, window))).await
}

async fn settle(fx: &mut Fx) {
    gather_all(&mut fx.actors, Duration::from_millis(700)).await;
}

/// Setup is done: let its notifications drain, then snapshot every list.
async fn ready(fx: &mut Fx) {
    settle(fx).await;
    let mut before = Vec::new();
    for a in &fx.actors {
        before.push(tool_names(&a.client).await);
    }
    fx.before = before;
}

async fn tool_names(client: &McpClient) -> Vec<String> {
    let listed = client.tools_list().await.expect("tools/list");
    let mut names: Vec<String> = listed["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
    names.sort();
    names
}

async fn fixture() -> Fx {
    let server = TestServer::start().await;
    let mut actors = Vec::new();
    for (i, who) in ["caller", "target", "bystander", "empty"].iter().enumerate() {
        let (ns, key) = signup(&server.base_url, &format!("sublis3 {who}")).await;
        let mut stream = ListenStream::open(&server.base_url, &key, 1000 + i as u64).await;
        assert_eq!(stream.status, 200);
        let (method, _) = stream.next_method(Duration::from_secs(5)).await.expect("acknowledgement");
        assert_eq!(method, "notifications/subscriptions/acknowledged");
        actors.push(Actor { client: McpClient::with_bearer(&server.base_url, &key), ns, stream });
    }
    Fx { server, actors, note_key: None, before: Vec::new() }
}

/// Setup (settled) then the one call the row is about. Returns who is who.
async fn drive(fx: &mut Fx, verb: &str) -> Ctx {
    let target_ns = fx.actors[TARGET].ns.clone();
    let caller_ns = fx.actors[CALLER].ns.clone();
    let c = McpClient::with_bearer(&fx.server.base_url, fx.actors[CALLER].client.bearer.as_deref().unwrap());
    let t = McpClient::with_bearer(&fx.server.base_url, fx.actors[TARGET].client.bearer.as_deref().unwrap());
    let in_group = |members: Vec<usize>| members;
    match verb {
        "host.tool.publish" => {
            ready(fx).await;
            call(&c, "host.tool.publish", json!({"name": "p1", "kind": "echo", "spec": echo_spec("a")})).await;
            Ctx { caller: CALLER, target: None, members: vec![] }
        }
        "host.tool.remove" => {
            call(&c, "host.tool.publish", json!({"name": "p1", "kind": "echo", "spec": echo_spec("a")})).await;
            ready(fx).await;
            call(&c, "host.tool.remove", json!({"name": "p1"})).await;
            Ctx { caller: CALLER, target: None, members: vec![] }
        }
        "host.tool.rollback" => {
            call(&c, "host.tool.publish", json!({"name": "p2", "kind": "echo", "spec": echo_spec("a")})).await;
            call(&c, "host.tool.publish", json!({"name": "p2", "kind": "echo", "spec": echo_spec("b")})).await;
            ready(fx).await;
            call(&c, "host.tool.rollback", json!({"name": "p2", "version": 1})).await;
            Ctx { caller: CALLER, target: None, members: vec![] }
        }
        "host.tool.fork" => {
            call(&t, "host.group.create", json!({"name": "gt"})).await;
            call(&t, "host.group.add", json!({"name": "gt", "namespace": caller_ns})).await;
            call(&t, "host.tool.publish", json!({"name": "s1", "kind": "echo", "spec": echo_spec("a")})).await;
            call(&t, "host.tool.share", json!({"name": "s1", "visibility": "group", "group": "gt", "scopes": ["call", "fork"]})).await;
            ready(fx).await;
            call(&c, "host.tool.fork", json!({"owner": target_ns, "tool": "s1", "as": "copy"})).await;
            Ctx { caller: CALLER, target: None, members: vec![] }
        }
        "host.import" => {
            call(&c, "host.tool.publish", json!({"name": "p1", "kind": "echo", "spec": echo_spec("a")})).await;
            let mut exporter = timp::Tenant { client: McpClient::with_bearer(&fx.server.base_url, c.bearer.as_deref().unwrap()) };
            let (run_id, _) = exporter.export(&fx.server).await;
            let _ = &mut exporter;
            ready(fx).await;
            let importer = timp::Tenant { client: McpClient::with_bearer(&fx.server.base_url, fx.actors[EMPTY].client.bearer.as_deref().unwrap()) };
            let done = importer.import_run(&run_id).await;
            assert_eq!(done["status"], json!("done"), "import failed: {done}");
            Ctx { caller: EMPTY, target: None, members: vec![] }
        }
        "host.example.fork" => {
            let examples = call(&c, "host.example.list", json!({})).await;
            let name = examples["examples"][0]["name"].as_str().expect("an example").to_string();
            ready(fx).await;
            call(&c, "host.example.fork", json!({"name": name})).await;
            Ctx { caller: CALLER, target: None, members: vec![] }
        }
        "host.tool.share" => {
            call(&c, "host.tool.publish", json!({"name": "p3", "kind": "echo", "spec": echo_spec("a")})).await;
            try_call(&c, "host.group.create", json!({"name": "g"})).await;
            try_call(&c, "host.group.add", json!({"name": "g", "namespace": target_ns})).await;
            ready(fx).await;
            call(&c, "host.tool.share", json!({"name": "p3", "visibility": "group", "group": "g"})).await;
            Ctx { caller: CALLER, target: None, members: in_group(vec![TARGET]) }
        }
        "host.tool.unshare" => {
            call(&c, "host.tool.publish", json!({"name": "p3", "kind": "echo", "spec": echo_spec("a")})).await;
            try_call(&c, "host.group.create", json!({"name": "g"})).await;
            try_call(&c, "host.group.add", json!({"name": "g", "namespace": target_ns})).await;
            call(&c, "host.tool.share", json!({"name": "p3", "visibility": "group", "group": "g"})).await;
            ready(fx).await;
            call(&c, "host.tool.unshare", json!({"name": "p3"})).await;
            Ctx { caller: CALLER, target: None, members: in_group(vec![TARGET]) }
        }
        "host.group.add" => {
            try_call(&c, "host.group.create", json!({"name": "g"})).await;
            try_call(&c, "host.group.remove", json!({"name": "g", "namespace": target_ns})).await;
            ready(fx).await;
            call(&c, "host.group.add", json!({"name": "g", "namespace": target_ns})).await;
            Ctx { caller: CALLER, target: Some(TARGET), members: vec![] }
        }
        "host.group.remove" => {
            try_call(&c, "host.group.create", json!({"name": "g"})).await;
            try_call(&c, "host.group.add", json!({"name": "g", "namespace": target_ns})).await;
            ready(fx).await;
            call(&c, "host.group.remove", json!({"name": "g", "namespace": target_ns})).await;
            Ctx { caller: CALLER, target: Some(TARGET), members: vec![] }
        }
        "host.note.put" => {
            call(&c, "host.tool.publish", json!({"name": "p3", "kind": "echo", "spec": echo_spec("a")})).await;
            ready(fx).await;
            let put = call(&c, "host.note.put", json!({"text": "remember", "attach_to": {"tool": "p3"}})).await;
            fx.note_key = put["key"].as_str().map(str::to_string);
            Ctx { caller: CALLER, target: None, members: vec![] }
        }
        "host.note.delete" => {
            call(&c, "host.tool.publish", json!({"name": "p3", "kind": "echo", "spec": echo_spec("a")})).await;
            let put = call(&c, "host.note.put", json!({"text": "remember", "attach_to": {"tool": "p3"}})).await;
            let key = put["key"].as_str().expect("note key").to_string();
            ready(fx).await;
            call(&c, "host.note.delete", json!({"key": key})).await;
            Ctx { caller: CALLER, target: None, members: vec![] }
        }
        other => panic!("TOOLS_LIST_OPS row `{other}` has no driver in sublis_ac03_drift: add one (or list it in DRIVEN_ELSEWHERE)"),
    }
}

/// The tenants a row's `affected` names, given who is who in this run.
fn expected(op: &ToolsListOp, ctx: &Ctx) -> Vec<usize> {
    if !op.effects.contains(&Effect::Tools) {
        return vec![];
    }
    let mut out = Vec::new();
    for affected in op.affected {
        match affected {
            Affected::Caller => out.push(ctx.caller),
            Affected::Target(_) => out.extend(ctx.target),
            Affected::GroupMembers(_) => out.extend(ctx.members.iter().copied()),
        }
    }
    out.sort();
    out.dedup();
    out
}

#[tokio::test]
async fn sublis_ac03_every_row_reaches_each_affected_tenant_once_and_no_other() {
    let mut fx = fixture().await;
    for op in TOOLS_LIST_OPS {
        if DRIVEN_ELSEWHERE.iter().any(|(verb, _)| *verb == op.verb) {
            continue;
        }
        let ctx = drive(&mut fx, op.verb).await;
        let seen = gather_all(&mut fx.actors, WINDOW).await;
        let want = expected(op, &ctx);
        for (i, methods) in seen.iter().enumerate() {
            let tools = methods.iter().filter(|m| *m == "notifications/tools/list_changed").count();
            let prompts = methods.iter().filter(|m| *m == "notifications/prompts/list_changed").count();
            let should = usize::from(want.contains(&i));
            assert_eq!(tools, should, "{}: actor {i} got {tools} tools/list_changed, wants {should} (affected {want:?}): {methods:?}", op.verb);
            assert_eq!(prompts, 0, "{}: a non-prompt tool sends no prompts/list_changed to actor {i}", op.verb);
        }
        // Whoever's tools/list really changed must have been told.
        for (i, a) in fx.actors.iter().enumerate() {
            if tool_names(&a.client).await != fx.before[i] {
                assert!(want.contains(&i), "{}: actor {i}'s tools/list changed but `affected` does not name it", op.verb);
            }
        }
        let _ = BYSTANDER;
    }
    for (verb, file) in DRIVEN_ELSEWHERE {
        assert!(TOOLS_LIST_OPS.iter().any(|op| op.verb == *verb), "{verb} is no longer a row");
        assert!(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file).exists(), "{file} drives {verb}");
    }
}

#[tokio::test]
async fn sublis_ac03_share_and_unshare_deliver_to_the_group_not_to_a_bystander() {
    let mut fx = fixture().await;
    for verb in ["host.tool.share", "host.tool.unshare"] {
        let ctx = drive(&mut fx, verb).await;
        let seen = gather_all(&mut fx.actors, WINDOW).await;
        let got: Vec<usize> = (0..seen.len())
            .filter(|i| seen[*i].iter().any(|m| m == "notifications/tools/list_changed"))
            .collect();
        assert_eq!(got, vec![CALLER, TARGET], "{verb}: the owner and the group member hear it, the bystander and the empty tenant do not");
        assert_eq!(ctx.members, vec![TARGET]);
    }
}

#[tokio::test]
async fn sublis_ac03_unlisted_verb_writing_a_tool_row_trips_the_debug_write_check() {
    if !cfg!(debug_assertions) {
        return;
    }
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "sublis3 unlisted").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("lookup").expect("tenant");
    let state = server.state.clone();
    let outcome = tokio::spawn(mcphost::listen::CURRENT_VERB.scope("test.unlisted".to_string(), async move {
        state.db.upsert_tool(tenant.id, "sneaky".into(), "echo".into(), echo_spec("a"), 5, vec![]).await
    }))
    .await;
    let panic = outcome.expect_err("an unlisted verb's tool write must panic in a debug build").into_panic();
    let message = panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
    assert!(message.contains("test.unlisted"), "names the verb: {message}");
    assert!(message.contains("TOOLS_LIST_OPS"), "names the table: {message}");

    // The same write under a listed verb, and with no verb at all, is fine.
    let state = server.state.clone();
    let listed = mcphost::listen::CURRENT_VERB
        .scope("host.tool.publish".to_string(), async move {
            state.db.upsert_tool(tenant.id, "fine".into(), "echo".into(), echo_spec("a"), 5, vec![]).await
        })
        .await;
    assert!(listed.is_ok(), "a listed verb may write tool rows");
    let no_verb = server.state.db.upsert_tool(tenant.id, "plain".into(), "echo".into(), echo_spec("a"), 5, vec![]).await;
    assert!(no_verb.is_ok(), "a write outside any verb is not checked");
}

#[test]
fn sublis_ac03_write_hint_and_prompt_tracking_read_the_table() {
    let args = json!({"name": "x", "as": "y", "namespace": "n", "group": "g"});
    for op in TOOLS_LIST_OPS {
        assert_eq!(
            mcphost::resources::write_hint(op.verb, &args).is_some(),
            op.effects.contains(&Effect::Resource),
            "{}: write_hint's Tool arm follows the Resource effect",
            op.verb
        );
        assert_eq!(
            mcphost::listen::tracks_prompts(op.verb),
            op.effects.contains(&Effect::Prompts),
            "{}: prompt tracking follows the Prompts effect",
            op.verb
        );
    }
    // An alias resolves to its row.
    assert!(mcphost::resources::write_hint("host.tool_publish", &args).is_some());
    // No verb outside the table has a Tool-resource hint.
    for verb in ["host.tool.list", "host.tool.test", "host.whoami"] {
        assert!(mcphost::listen::op_for(verb).is_none(), "{verb} is not a row");
    }
}

#[tokio::test]
async fn sublis_ac03_a_prompt_kind_publish_also_sends_prompts_list_changed() {
    let kinds = PromptKind::new().registry();
    let server = TestServer::start_with_kinds(kinds).await;
    let (_ns, key) = signup(&server.base_url, "sublis3 prompts").await;
    let mut stream = ListenStream::open(&server.base_url, &key, 77).await;
    let (method, _) = stream.next_method(Duration::from_secs(5)).await.expect("ack");
    assert_eq!(method, "notifications/subscriptions/acknowledged");
    let client = McpClient::with_bearer(&server.base_url, &key);
    let spec = json!({"template": "Classify: {{ticket}}", "args_schema": {"type": "object", "properties": {"ticket": {"type": "string"}}, "required": ["ticket"]}});
    call(&client, "host.tool.publish", json!({"name": "classify_ticket", "kind": "prompt", "spec": spec})).await;
    let mut methods = gather(&mut stream, WINDOW).await;
    methods.sort();
    assert_eq!(methods, vec!["notifications/prompts/list_changed", "notifications/tools/list_changed"], "{methods:?}");
}
