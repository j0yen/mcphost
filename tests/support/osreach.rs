//! Shared engine of the `osreach_ac*` proofs (PRD-mcphost-output-schema-reach-sweep-live):
//! one sweep drives every name `tools/list` serves through real dispatch in its
//! non-live states, validates each success `structuredContent` against the
//! `outputSchema` exactly as served, and records which `(tool, state)` pairs it
//! could not reach. The sweep runs once per suite binary; every AC reads the
//! same `Sweep`. No tool name is spelled in this file: names come from
//! `tools/list`, kinds from `mcphost::write_verbs`.
//!
//! `allow(dead_code)`: each including test file uses only part of this.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::common::{McpClient, TestServer, signup};
use mcphost::write_verbs::{DryRun, Kind, WRITE_VERBS};
use serde_json::{Map, Value, json};
use tokio::sync::OnceCell;

pub const STATE_FRESH: &str = "fresh";
pub const STATE_DRY_RUN: &str = "dry_run";
pub const STATE_AFTER_CREATE: &str = "after_create";
pub const VARIANT_DRY_RUN: &str = "dry_run";
pub const VARIANT_OWN_DRY_RUN: &str = "own_dry_run";
pub const VARIANT_EMPTY: &str = "empty";

const CALL_TIMEOUT: Duration = Duration::from_secs(20);

pub const VARIANTS_PATH: &str = "contracts/host-output-variants.v1.json";
pub const SCHEMAS_PATH: &str = "contracts/host-output-schemas.v1.json";
pub const EDITS_PATH: &str = "contracts/host-output-observational-edits.v1.json";

pub fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// One name `tools/list` serves, with both schemas exactly as served.
#[derive(Debug, Clone)]
pub struct Served {
    pub name: String,
    pub input: Value,
    pub output: Option<Value>,
}

impl Served {
    pub fn declares_dry_run(&self) -> bool {
        self.input.pointer("/properties/dry_run").is_some()
    }
}

/// What the sweep saw for one tool.
#[derive(Debug, Clone, Default)]
pub struct ToolRow {
    pub calls: usize,
    pub successes: usize,
    pub variants: BTreeSet<String>,
    /// `(state, error code)` of every state that never produced a success.
    pub unreached: BTreeMap<String, String>,
    /// Validation failures: `tool state instance_path keyword`.
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Sweep {
    pub served: Vec<Served>,
    pub rows: BTreeMap<String, ToolRow>,
}

impl Sweep {
    pub fn unreached_count(&self) -> usize {
        self.rows.values().map(|r| r.unreached.len()).sum()
    }

    pub fn failures(&self) -> Vec<String> {
        self.rows.values().flat_map(|r| r.failures.clone()).collect()
    }

    /// The per-tool lines the sweep prints.
    pub fn lines(&self) -> Vec<String> {
        self.rows.iter().map(|(tool, row)| line_for(tool, row)).collect()
    }
}

pub fn line_for(tool: &str, row: &ToolRow) -> String {
    let variants: Vec<&str> = row.variants.iter().map(String::as_str).collect();
    let unreached: Vec<String> = row.unreached.iter().map(|(s, c)| format!("{s}:{c}")).collect();
    format!(
        "osreach {tool}: calls={} successes={} variants=[{}] unreached=[{}]",
        row.calls,
        row.successes,
        variants.join(","),
        unreached.join(",")
    )
}

/// The validator path: `Err` names the tool, the state, the instance path and
/// the failing keyword of the first mismatch.
pub fn check(tool: &str, state: &str, schema: &Value, instance: &Value) -> Result<(), String> {
    let validator = jsonschema::validator_for(schema).map_err(|e| format!("{tool} {state}: schema does not compile: {e}"))?;
    let Some(first) = validator.iter_errors(instance).next() else { return Ok(()) };
    let mut keyword = keyword_of(&first.schema_path.to_string());
    let mut path = first.instance_path.to_string();
    let mut detail = String::new();
    if keyword == "anyOf" {
        // The root `anyOf` hides the branch that nearly matched; name the first
        // mismatch against the live shape too.
        let live = mcphost::output_schemas::live_view(schema);
        if let Ok(v) = jsonschema::validator_for(&live)
            && let Some(e) = v.iter_errors(instance).next()
        {
            detail = format!(" (live branch: {} {})", e.instance_path, keyword_of(&e.schema_path.to_string()));
            if path.is_empty() {
                path = e.instance_path.to_string();
            }
        }
    }
    if path.is_empty() {
        path = "/".to_string();
    }
    if keyword.is_empty() {
        keyword = "schema".to_string();
    }
    Err(format!("{tool} {state} instance_path={path} keyword={keyword}{detail}: {first}"))
}

fn keyword_of(schema_path: &str) -> String {
    schema_path.rsplit('/').find(|s| !s.is_empty() && s.parse::<usize>().is_err()).unwrap_or("").to_string()
}

/// Which variant a success result is.
pub fn variant_of(state: &str, structured: &Value) -> Option<&'static str> {
    let obj = structured.as_object()?;
    if obj.len() == 1 && obj.contains_key(mcphost::output_schemas::EMPTY_KEY) {
        return Some(VARIANT_EMPTY);
    }
    if state == STATE_DRY_RUN && structured.pointer("/_meta/dry_run") == Some(&json!(true)) {
        return Some(if obj.contains_key("would") { VARIANT_DRY_RUN } else { VARIANT_OWN_DRY_RUN });
    }
    if state == STATE_DRY_RUN && obj.get("dry_run") == Some(&json!(true)) {
        return Some(VARIANT_OWN_DRY_RUN);
    }
    None
}

/// Names that declare `dry_run`, are not a `WRITE_VERBS` row, and have no
/// generated `own_dry_run` branch.
pub fn undeclared_dry_run(served: &[Served]) -> Vec<String> {
    let canonical = canonical_of;
    served
        .iter()
        .filter(|s| s.declares_dry_run())
        .map(|s| canonical(&s.name))
        .filter(|c| mcphost::write_verbs::lookup(c).is_none() && !mcphost::output_schemas::has_own_dry_run(c))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The canonical name a served name (alias or flattened form) stands for.
pub fn canonical_of(name: &str) -> String {
    use std::sync::OnceLock;
    static MAP: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    let map = MAP.get_or_init(|| {
        let mut m = BTreeMap::new();
        for c in mcphost::handler::canonical_control_plane_names(&mcphost::kinds::KindRegistry::with_builtin()) {
            if let Some(f) = mcphost::tool_aliases::flattened_form(&c) {
                m.insert(f, c.clone());
            }
            for a in mcphost::tool_aliases::aliases_of(&c) {
                m.insert(a.to_string(), c.clone());
            }
        }
        m
    });
    map.get(name).cloned().unwrap_or_else(|| name.to_string())
}

fn is_create_kind(canonical: &str) -> bool {
    WRITE_VERBS
        .iter()
        .any(|v| v.name == canonical && matches!(v.kind, Kind::Write | Kind::Publish) && matches!(v.dry_run, DryRun::Supported))
}

fn fill(args: &mut Value, harvested: &BTreeMap<String, Value>) {
    if let Some(obj) = args.as_object_mut() {
        for (k, v) in obj.iter_mut() {
            if let Some(h) = harvested.get(k) {
                *v = h.clone();
            }
        }
    }
}

/// `id`-like scalars of a create result, keyed by their field name.
fn harvest(structured: &Value, into: &mut BTreeMap<String, Value>) {
    if let Some(obj) = structured.as_object() {
        for (k, v) in obj {
            if matches!(v, Value::String(_) | Value::Number(_)) && !k.starts_with('_') {
                into.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
    }
}

enum Outcome {
    Success(Value),
    Failed(String),
}

/// Error codes a second attempt may clear: the host was busy, not the state
/// unreachable. Anything else is final on the first answer.
const TRANSIENT: &[&str] = &["storage", "timeout"];

async fn call(client: &McpClient, name: &str, args: Value) -> Outcome {
    let mut outcome = call_once(client, name, args.clone()).await;
    for _ in 0..3 {
        match &outcome {
            Outcome::Failed(code) if TRANSIENT.contains(&code.as_str()) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                outcome = call_once(client, name, args.clone()).await;
            }
            _ => break,
        }
    }
    outcome
}

async fn call_once(client: &McpClient, name: &str, args: Value) -> Outcome {
    match tokio::time::timeout(CALL_TIMEOUT, client.tools_call(name, args)).await {
        Err(_) => Outcome::Failed("timeout".to_string()),
        Ok(Err(e)) => Outcome::Failed(e.error_code.unwrap_or_else(|| format!("rpc{}", e.code))),
        Ok(Ok(result)) => {
            if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
                let code = result
                    .pointer("/structuredContent/error/code")
                    .or_else(|| result.pointer("/structuredContent/code"))
                    .and_then(Value::as_str)
                    .unwrap_or("tool_error");
                return Outcome::Failed(code.to_string());
            }
            match result.get("structuredContent") {
                Some(s) if s.is_object() => Outcome::Success(s.clone()),
                _ => Outcome::Failed("no_structured_content".to_string()),
            }
        }
    }
}

async fn list(client: &McpClient) -> Vec<Served> {
    let listed = client.tools_list().await.expect("tools/list");
    listed["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| Served {
            name: t["name"].as_str().expect("name").to_string(),
            input: t.get("inputSchema").cloned().unwrap_or_else(|| json!({})),
            output: t.get("outputSchema").cloned(),
        })
        .collect()
}

fn record(row: &mut ToolRow, s: &Served, state: &str, outcome: Outcome) {
    row.calls += 1;
    match outcome {
        Outcome::Failed(code) => {
            row.unreached.entry(state.to_string()).or_insert(code);
        }
        Outcome::Success(structured) => {
            row.successes += 1;
            row.unreached.remove(state);
            if let Some(v) = variant_of(state, &structured) {
                row.variants.insert(v.to_string());
            }
            if let Some(schema) = &s.output
                && let Err(msg) = check(&s.name, state, schema, &structured)
            {
                row.failures.push(msg);
            }
        }
    }
}

async fn fresh_client(server: &TestServer) -> McpClient {
    let (_ns, key) = signup(&server.base_url, "osreach").await;
    McpClient::with_bearer(&server.base_url, &key)
}

/// Runs the whole sweep on a throwaway server.
pub async fn run_sweep() -> Sweep {
    let server = TestServer::start_with_signup_rate_limit(1_000_000).await;
    let lister = fresh_client(&server).await;
    let served: Vec<Served> = list(&lister).await.into_iter().filter(|s| s.name != "signup").collect();
    let mut sweep = Sweep { served: served.clone(), rows: BTreeMap::new() };
    let creators: Vec<&Served> = served.iter().filter(|s| is_create_kind(&canonical_of(&s.name))).collect();
    // A populated tenant shared by every read-only name: each create-kind verb
    // called once, ids passed on by argument name.
    let (shared, shared_ids) = populate(&server, &creators).await;
    for s in &served {
        let canonical = canonical_of(&s.name);
        let mut row = ToolRow::default();
        let args = crate::vbd_calls::call_for(s.input.as_object().unwrap_or(&Map::new()));
        // state 1: fresh tenant
        let client = fresh_client(&server).await;
        record(&mut row, s, STATE_FRESH, call(&client, &s.name, args.clone()).await);
        // state 2: dry_run: true
        if s.declares_dry_run() {
            let client = fresh_client(&server).await;
            let mut a = args.clone();
            a["dry_run"] = json!(true);
            record(&mut row, s, STATE_DRY_RUN, call(&client, &s.name, a).await);
        }
        // state 3: after the create-kind verbs ran
        let mut a = args.clone();
        let destructive = mcphost::write_verbs::lookup(&canonical).is_some() || canonical.starts_with("host.tenant");
        if destructive {
            let (client, ids) = populate(&server, &creators).await;
            fill(&mut a, &ids);
            record(&mut row, s, STATE_AFTER_CREATE, call(&client, &s.name, a).await);
        } else {
            fill(&mut a, &shared_ids);
            record(&mut row, s, STATE_AFTER_CREATE, call(&shared, &s.name, a).await);
        }
        sweep.rows.insert(s.name.clone(), row);
    }
    sweep
}

async fn populate(server: &TestServer, creators: &[&Served]) -> (McpClient, BTreeMap<String, Value>) {
    let client = fresh_client(server).await;
    let mut ids = BTreeMap::new();
    for s in creators {
        let mut a = crate::vbd_calls::call_for(s.input.as_object().unwrap_or(&Map::new()));
        fill(&mut a, &ids);
        if let Outcome::Success(structured) = call(&client, &s.name, a).await {
            harvest(&structured, &mut ids);
        }
    }
    (client, ids)
}

static SWEEP: OnceCell<Sweep> = OnceCell::const_new();

/// The sweep, run once per test binary.
pub async fn sweep() -> &'static Sweep {
    SWEEP.get_or_init(run_sweep).await
}

pub fn variants_file() -> PathBuf {
    root().join(VARIANTS_PATH)
}

// ---- the variants record ----

/// `OSREACH_WRITE=1`: the sweep rewrites the variants file.
pub fn write_mode() -> bool {
    std::env::var("OSREACH_WRITE").is_ok_and(|v| v == "1")
}

/// The `unreached_cap` of a committed variants file under `dir`, `None` when
/// the file does not exist yet.
pub fn committed_cap(dir: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(dir.join(VARIANTS_PATH)).ok()?;
    serde_json::from_str::<Value>(&text).ok()?.get("unreached_cap")?.as_u64()
}

/// The cap write mode commits: the lower of the committed cap and the count
/// seen, never above the committed one; the first write sets it to the count.
pub fn next_cap(committed: Option<u64>, count: u64) -> u64 {
    committed.map_or(count, |c| c.min(count))
}

/// The variants record the sweep would write, under `cap`.
pub fn render(sweep: &Sweep, cap: u64) -> Value {
    let tools: Map<String, Value> = sweep
        .rows
        .iter()
        .map(|(tool, row)| {
            let unreached: Vec<Value> =
                row.unreached.iter().map(|(state, code)| json!({"state": state, "code": code})).collect();
            (tool.clone(), json!({"variants": row.variants, "unreached": unreached}))
        })
        .collect();
    json!({"unreached_cap": cap, "tools": tools})
}

pub fn to_text(record: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(record).expect("record serializes"))
}

/// Every place `expected` and `actual` differ, as `tool <name> at <JSON pointer>`.
pub fn differences(expected: &Value, actual: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk(expected, actual, "", &mut out);
    out
}

fn walk(expected: &Value, actual: &Value, pointer: &str, out: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for key in e.keys().chain(a.keys()).collect::<BTreeSet<_>>() {
                let child = format!("{pointer}/{key}");
                match (e.get(key), a.get(key)) {
                    (Some(x), Some(y)) => walk(x, y, &child, out),
                    _ => out.push(describe(&child)),
                }
            }
        }
        _ if expected != actual => out.push(describe(pointer)),
        _ => {}
    }
}

fn describe(pointer: &str) -> String {
    match pointer.strip_prefix("/tools/").and_then(|rest| rest.split('/').next()) {
        Some(tool) => format!("tool {tool} at {pointer}"),
        None => format!("at {pointer}"),
    }
}

/// Write mode rewrites only the variants file under `dir`; compare mode fails
/// naming each tool and JSON pointer that differs from the committed file.
pub fn sync_variants(sweep: &Sweep, dir: &Path, write: bool) -> Result<(), String> {
    let committed = committed_cap(dir);
    let path = dir.join(VARIANTS_PATH);
    if write {
        let record = render(sweep, next_cap(committed, sweep.unreached_count() as u64));
        let text = to_text(&record);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
        }
        return Ok(());
    }
    let on_disk: Value = std::fs::read_to_string(&path)
        .map_err(|e| format!("{VARIANTS_PATH} is not committed ({e}); run with OSREACH_WRITE=1"))
        .and_then(|t| serde_json::from_str(&t).map_err(|e| format!("{VARIANTS_PATH} does not parse: {e}")))?;
    let expected = render(sweep, committed.unwrap_or(0));
    let diffs = differences(&expected, &on_disk);
    if diffs.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{VARIANTS_PATH} differs from the sweep in {} place(s); regenerate with OSREACH_WRITE=1:\n{}",
        diffs.len(),
        diffs.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    ))
}

/// R3: the count must not exceed the committed cap.
pub fn within_cap(count: u64, cap: u64) -> Result<(), String> {
    if count > cap {
        return Err(format!("{count} unreached (tool, state) pairs exceed the committed unreached_cap {cap}; the cap only moves down"));
    }
    Ok(())
}

/// R4: a served name outside `WRITE_VERBS` that declares `dry_run` and has no
/// generated `own_dry_run` branch fails when its dry-run result is not admitted
/// by the served schema; the error names each such tool.
pub fn require_dry_run_branches(served: &[Served], rows: &BTreeMap<String, ToolRow>) -> Result<(), String> {
    let candidates: BTreeSet<String> = undeclared_dry_run(served).into_iter().collect();
    let rejected: BTreeSet<String> = served
        .iter()
        .filter(|s| candidates.contains(&canonical_of(&s.name)))
        .filter(|s| rows.get(&s.name).is_some_and(|r| r.failures.iter().any(|f| f.contains(&format!(" {STATE_DRY_RUN} ")))))
        .map(|s| canonical_of(&s.name))
        .collect();
    if rejected.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} tool(s) declare dry_run outside WRITE_VERBS, have no generated own_dry_run branch, and their dry-run result is rejected: {}",
        rejected.len(),
        rejected.into_iter().collect::<Vec<_>>().join(", ")
    ))
}
