//! PRD-mcphost-tool-call-host-verb-forward, stack-budget guard (hand PR
//! hand/forward-stack-budget, after the two 10-17 debug-build stack overflows,
//! run 326 and run 953).
//!
//! Every dotted host verb `host.tool_call` can forward (the forwardable set is
//! DERIVED: `handler::host_tool_descriptors` minus aliases minus
//! `verbforward::FORWARD_DENYLIST`, the same walk `verbforward::resolve` does)
//! must complete its real forwarded call on a thread whose stack is only
//! `handler::FORWARD_STACK_BUDGET` (measured max + 25 %, under CI's 2 MiB test
//! thread), so a new `.await` or a fatter frame anywhere on the chain fails HERE,
//! naming the verb, instead of as a SIGABRT of whatever shard happens to run it.
//!
//! A stack overflow is SIGABRT, not a panic: it kills the whole test binary, so
//! the real calls run in child processes (this same test binary re-executed on
//! this same test, selected by `CHILD_VERBS_ENV`). The child runs one verb at a
//! time, each on a fresh thread of exactly the budgeted stack, with its own
//! current-thread runtime and server (the setup `verbfwd_ac01` uses), and
//! prints `FWD-START <id>` before and `FWD-DONE <id>` after. When a child dies
//! the parent names the last started verb, then re-runs the verbs after it in a
//! new child, so one run reports EVERY overflowing verb.
//!
//! To measure a verb's minimum stack instead of testing the budget, run the
//! test binary directly as a child:
//! `MCPHOST_FWD_STACK_VERBS=call:host.trigger.set MCPHOST_FWD_STACK_BYTES=700000 \
//!  cargo test --test suite_core_08 -- --exact <this test> --nocapture`
//! (exit status 134 / SIGABRT = overflow; `FWD-DONE` = fits).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::handler::FORWARD_STACK_BUDGET;
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};

/// Child mode: comma-separated `<call|test>:<canonical verb>` ids to run, or `all`.
const CHILD_VERBS_ENV: &str = "MCPHOST_FWD_STACK_VERBS";
/// Stack size in bytes for each verb's thread; unset = the budget.
const CHILD_STACK_ENV: &str = "MCPHOST_FWD_STACK_BYTES";

/// Ids whose forwarded call still stops at ARGUMENT VALIDATION, because the verb
/// needs something its schema does not say (`id` or `name`; `schedule` when
/// `kind` is `schedule`; a non-empty list). Each still crosses the forwarding
/// hop, which is what the budget is about, but the verb's own body is not
/// measured. The list only shrinks: the test fails if an id outside it stops at
/// validation, and fails if an id on it no longer does (drop the entry), and
/// `SHALLOW_CEILING` is its length today. The real fix is the schema declaring
/// those conditional requirements, so the arguments can be generated.
const SHALLOW: &[&str] = &[
    "call:host.agent.contacts_import",
    "call:host.ask.create",
    "call:host.blob.get",
    "call:host.blob.put",
    "call:host.call_trace",
    "call:host.channel.open",
    "call:host.docs.delete",
    "call:host.docs.get",
    "call:host.docs.index_config",
    "call:host.docs.purge",
    "call:host.docs.put",
    "call:host.example.propose",
    "call:host.import",
    "call:host.index.create",
    "call:host.key.delegate",
    "call:host.key.mint",
    "call:host.lineage.blast_radius",
    "call:host.msg.send",
    "call:host.note.delete",
    "call:host.org.create",
    "call:host.policy.set",
    "call:host.share.caller_limit",
    "call:host.state.table_create",
    "call:host.table.create",
    "call:host.tool.spec_shared",
    "call:host.trigger.fire",
    "call:host.trigger.get",
    "call:host.trigger.pause",
    "call:host.trigger.remove",
    "call:host.trigger.replay",
    "call:host.trigger.resume",
    "call:host.trigger.test",
    "call:host.uptime.create",
    "call:host.vault.provider_set",
];
const SHALLOW_CEILING: usize = 34;

/// The test fn's name as libtest lists it: a suite binary nests this file under
/// its module (`verbfwd_ac08_forward_stack_budget::<fn>`), a standalone one does not.
fn libtest_name() -> String {
    const FN: &str = "every_dotted_verb_forwards_within_the_stack_budget";
    match module_path!().split_once("::") {
        Some((_crate, module)) => format!("{module}::{FN}"),
        None => FN.to_string(),
    }
}

/// The string every generated argument uses; also the name of the one tool each
/// tenant publishes (tool names are `^[a-z][a-z0-9_]{1,40}$`, so two letters).
const PLACEHOLDER: &str = "xx";

/// A minimal valid-looking value for a JSON-schema fragment: first enum
/// member, else first `examples` entry, else a type-appropriate scalar, objects carrying their required
/// properties. Generated from the descriptor, never listed by hand.
fn sample(schema: &Value) -> Value {
    if let Some(first) = schema.get("enum").and_then(Value::as_array).and_then(|a| a.first()) {
        return first.clone();
    }
    // A property whose shape a type cannot say (a domain name) publishes `examples`
    // from the same const its validator is tested against.
    if let Some(first) = schema.get("examples").and_then(Value::as_array).and_then(|a| a.first()) {
        return first.clone();
    }
    // `host.email.send`'s `to` declares `format: email`: a string with no `@` stops at
    // argument validation, so the verb's own body would never be measured.
    if schema.get("format").and_then(Value::as_str) == Some("email") {
        return json!(format!("{PLACEHOLDER}@{PLACEHOLDER}.example"));
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(first) = schema.get(key).and_then(Value::as_array).and_then(|a| a.first()) {
            return sample(first);
        }
    }
    let ty = match schema.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or("null"),
        _ => "string",
    };
    match ty {
        "string" => {
            let min = schema.get("minLength").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
            json!(PLACEHOLDER.repeat(min))
        }
        "integer" => json!(schema.get("minimum").and_then(Value::as_i64).unwrap_or(1)),
        "number" => json!(schema.get("minimum").and_then(Value::as_f64).unwrap_or(1.0)),
        "boolean" => json!(false),
        "array" => json!([]),
        "object" => required_args(schema),
        _ => Value::Null,
    }
}

/// `{required property: sample(property schema)}` for an object schema.
/// `tenant_key` is never set: the client's bearer is the auth.
fn required_args(schema: &Value) -> Value {
    let props = schema.get("properties").and_then(Value::as_object);
    let mut out = serde_json::Map::new();
    for name in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if name != "tenant_key" {
            let prop = props.and_then(|p| p.get(name)).cloned().unwrap_or(Value::Null);
            out.insert(name.to_string(), sample(&prop));
        }
    }
    Value::Object(out)
}

/// The arguments one verb's forwarded call carries: generated from its descriptor's
/// `inputSchema.required`, except where the schema cannot say what a call needs.
fn args_for(tool: &rmcp::model::Tool) -> Value {
    // `host.trigger.set` is the verb behind both incidents and its schema does not
    // say that `schedule` is required for `kind: schedule`: these are the
    // arguments `verbfwd_ac02` forwards (its `tool` is the placeholder tool).
    if tool.name == "host.trigger.set" {
        return json!({"tool": PLACEHOLDER, "kind": "schedule", "schedule": "*/5 * * * *"});
    }
    required_args(&Value::Object((*tool.input_schema).clone()))
}

/// Every forwardable canonical dotted verb with its generated arguments. The
/// walk is `verbforward::resolve`'s: `host.*` descriptors that are not alias
/// spellings, minus the four denylisted verbs `host.tool_call` refuses.
fn forwardable_verbs() -> Vec<(String, Value)> {
    let kinds = KindRegistry::with_builtin();
    mcphost::handler::host_tool_descriptors(&kinds)
        .into_iter()
        .filter(|t| t.name.starts_with("host.") && mcphost::tool_aliases::resolve(&t.name).is_none())
        .filter(|t| !mcphost::verbforward::is_denylisted(&t.name))
        .map(|t| (t.name.to_string(), args_for(&t)))
        .collect()
}

/// The ids the guard runs: `call:<verb>` (host.tool_call forwards it) for every
/// forwardable verb, plus `test:<verb>` (host.tool_test forwards it) for every
/// verb with a dry run (`verbforward::DRY_RUN_FORWARDABLE`).
fn ids(verbs: &[(String, Value)]) -> Vec<String> {
    let mut out = Vec::new();
    for (verb, _) in verbs {
        out.push(format!("call:{verb}"));
        if mcphost::verbforward::supports_dry_run(verb) {
            out.push(format!("test:{verb}"));
        }
    }
    out
}

/// One real forwarded call, on the current (small-stack) thread's runtime.
async fn run_one(id: &str, args: Value) -> String {
    let (via, verb) = id.split_once(':').expect("id is <call|test>:<verb>");
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Stack Budget Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    // One published tool named `xx`, the placeholder every generated string
    // argument uses, so a verb that takes a tool name (`host.trigger.set`,
    // `host.tool.run`) gets past its lookup instead of failing it.
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": PLACEHOLDER, "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish the placeholder tool");
    // ...and one secret named `xx`, so `host.secret.leak_test` (which takes the
    // name of an existing secret) gets past its lookup and its body is measured.
    client
        .tools_call("host.secret.set", json!({"name": PLACEHOLDER, "value": PLACEHOLDER}))
        .await
        .expect("set the placeholder secret");
    // The wrapper's `name` is the dotted verb as an agent writes it (no `host.`).
    let name = verb.strip_prefix("host.").unwrap_or(verb);
    let wrapper = if via == "test" { "host.tool_test" } else { "host.tool_call" };
    let call = client.tools_call(wrapper, json!({"name": name, "args": args}));
    match tokio::time::timeout(std::time::Duration::from_secs(30), call).await {
        Err(_) => "timeout".to_string(),
        Ok(Ok(v)) => {
            let forwarded = extract_structured(&v)["forwarded_to"].as_str().map(str::to_string);
            format!("ok forwarded_to={}", forwarded.as_deref().unwrap_or("-"))
        }
        // An application error is fine: the call walked the whole chain.
        Ok(Err(e)) => {
            let code = e.error_code.clone().unwrap_or_else(|| e.code.to_string());
            let why: String = e.message.chars().filter(|c| *c != '\n').take(100).collect();
            format!("err {code}: {why}")
        }
    }
}

/// Child mode: run each id on a fresh thread of `stack` bytes.
fn run_child(list: &str, stack: usize, all: &[(String, Value)]) {
    let out = std::io::stdout();
    for id in list.split(',').filter(|s| !s.is_empty()) {
        let verb = id.split_once(':').map_or(id, |(_, v)| v);
        let args = all.iter().find(|(v, _)| v == verb).map(|(_, a)| a.clone()).unwrap_or_else(|| json!({}));
        {
            let mut o = out.lock();
            writeln!(o, "FWD-START {id}").expect("stdout");
            o.flush().expect("flush");
        }
        let id_owned = id.to_string();
        let joined = std::thread::Builder::new()
            .name(format!("fwd-{id}"))
            .stack_size(stack)
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("runtime");
                rt.block_on(run_one(&id_owned, args))
            })
            .expect("spawn")
            .join();
        let mut o = out.lock();
        match joined {
            Ok(outcome) => writeln!(o, "FWD-DONE {id} {outcome}").expect("stdout"),
            Err(_) => writeln!(o, "FWD-PANIC {id}").expect("stdout"),
        }
        o.flush().expect("flush");
    }
}

/// What one child-process worker found over its slice of the ids.
#[derive(Default)]
struct Found {
    overflowed: Vec<String>,
    other: Vec<String>,
    ran: usize,
    /// Ids whose call returned `args_invalid` / `invalid_params`.
    shallow: BTreeSet<String>,
}

/// Runs `pending` in child processes (this test binary re-executed), restarting
/// after each crash with the ids that have not run yet, so every overflowing
/// verb in the slice is named.
fn run_slice(exe: &std::path::Path, mut pending: Vec<String>) -> Found {
    let mut found = Found::default();
    while !pending.is_empty() {
        let out = Command::new(exe)
            .args(["--exact", &libtest_name(), "--nocapture", "--test-threads=1"])
            .env(CHILD_VERBS_ENV, pending.join(","))
            .env(CHILD_STACK_ENV, FORWARD_STACK_BUDGET.to_string())
            .env("RUST_BACKTRACE", "0")
            .stdin(Stdio::null())
            .output()
            .expect("spawn child test binary");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut started: Option<String> = None;
        let mut finished: BTreeSet<String> = BTreeSet::new();
        for line in stdout.lines() {
            // libtest prints `test <name> ... ` without a newline before the child's
            // first marker, so the first marker is not at the start of its line.
            let line = line.find("FWD-").map_or(line, |i| &line[i..]);
            if let Some(id) = line.strip_prefix("FWD-START ") {
                started = Some(id.to_string());
            } else if let Some(rest) = line.strip_prefix("FWD-DONE ") {
                let (id, outcome) = rest.split_once(' ').unwrap_or((rest, ""));
                finished.insert(id.to_string());
                found.ran += 1;
                if outcome.starts_with("err args_invalid") || outcome.starts_with("err invalid_params") {
                    found.shallow.insert(id.to_string());
                }
            } else if let Some(id) = line.strip_prefix("FWD-PANIC ") {
                finished.insert(id.to_string());
                found.other.push(format!("{id}: the forwarded call panicked on the {FORWARD_STACK_BUDGET}-byte stack"));
            }
        }
        let culprit = started.filter(|s| !finished.contains(s));
        pending.retain(|id| !finished.contains(id));
        if out.status.success() && culprit.is_none() {
            if !pending.is_empty() {
                found.other.push(format!(
                    "child ran nothing for {} id(s) (first: {}): is `{}` still the test's libtest name?",
                    pending.len(),
                    pending[0],
                    libtest_name()
                ));
            }
            break;
        }
        match culprit {
            Some(id) => {
                let how = match out.status.signal() {
                    Some(6) => "overflowed its stack (SIGABRT)".to_string(),
                    Some(11) => "overflowed its stack (SIGSEGV)".to_string(),
                    Some(s) => format!("was killed by signal {s}"),
                    None => format!("exited {:?}", out.status.code()),
                };
                found.overflowed.push(format!("{id} {how} on the {FORWARD_STACK_BUDGET}-byte budget"));
                pending.retain(|p| p != &id);
            }
            None => {
                let tail: String =
                    String::from_utf8_lossy(&out.stderr).lines().rev().take(8).collect::<Vec<_>>().join(" | ");
                found.other.push(format!("child died outside any verb ({:?}): {tail}", out.status));
                break;
            }
        }
    }
    found
}

#[test]
fn every_dotted_verb_forwards_within_the_stack_budget() {
    let verbs = forwardable_verbs();

    if let Ok(list) = std::env::var(CHILD_VERBS_ENV) {
        let stack = std::env::var(CHILD_STACK_ENV)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(FORWARD_STACK_BUDGET);
        let list = if list == "all" { ids(&verbs).join(",") } else { list };
        run_child(&list, stack, &verbs);
        return;
    }

    assert!(!verbs.is_empty(), "no forwardable host verb found: the descriptor walk is broken");
    assert!(SHALLOW.len() <= SHALLOW_CEILING, "SHALLOW may only shrink: {} entries > ceiling {SHALLOW_CEILING}", SHALLOW.len());

    // Every id is a server start + signup + call: spread them over a few child
    // processes at a time (the budget is per thread, so this changes nothing but
    // wall time).
    let exe = std::env::current_exe().expect("current_exe");
    let all = ids(&verbs);
    let total = all.len();
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
    let found: Vec<Found> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|w| {
                let slice: Vec<String> = all.iter().skip(w).step_by(workers).cloned().collect();
                let exe = &exe;
                scope.spawn(move || run_slice(exe, slice))
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("slice worker")).collect()
    });

    let ran: usize = found.iter().map(|f| f.ran).sum();
    let shallow: BTreeSet<String> = found.iter().flat_map(|f| f.shallow.iter().cloned()).collect();
    let mut failures: Vec<String> = found
        .iter()
        .flat_map(|f| f.overflowed.iter().chain(f.other.iter()))
        .cloned()
        .collect();
    failures.sort();
    assert!(
        failures.is_empty(),
        "forwarded host verbs must run in {FORWARD_STACK_BUDGET} bytes of stack (run 326 / 953, 10-17): \
         {} of {total} ids overflowed or failed ({ran} completed):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );

    // Only reached when nothing overflowed (a crashed child never reports its
    // outcome, so the shallow set is only complete then).
    let listed: BTreeSet<String> = SHALLOW.iter().map(|s| s.to_string()).collect();
    let new_shallow: Vec<&String> = shallow.difference(&listed).collect();
    let runs_now: Vec<&String> = listed.difference(&shallow).collect();
    assert!(
        new_shallow.is_empty(),
        "these ids stop at argument validation with the arguments generated from their schema, so their \
         own body is not measured; make their schema say what they need (or, as a last resort, add them \
         to SHALLOW): {new_shallow:?}"
    );
    assert!(runs_now.is_empty(), "these SHALLOW ids now run past argument validation: drop them from SHALLOW: {runs_now:?}");
}
