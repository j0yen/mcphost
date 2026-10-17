//! PRD-mcphost-example-proof-nightly-and-hide: every example's proof runs
//! against prod nightly (`mcphost examples prove --live <url> --report`) and
//! posts one `admin.example_proof_report` per example. The host stores the
//! verdict in `example_proofs`; a red proof sets `hidden_reason`, which
//! `host.example.list` and `host.example.fork` honor at once.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::client::{RpcRequest, extract_structured, rpc_call};
use crate::db::ExampleProofRow;
use crate::errors::AppError;
use crate::examples::{EXAMPLES, Example};
use crate::state::{AppState, now_unix, rfc3339_from_unix};

/// Wall-clock limit for one proof (R1).
pub const PROOF_TIMEOUT: Duration = Duration::from_secs(300);
/// Trailing log lines kept as the excerpt (R1).
pub const EXCERPT_LINES: usize = 20;

/// The spawn contract (R1, R5): every proof is run as `bash <proof> <url>`
/// with these env vars. `run_proof` and `contract_text` both read them.
pub const URL_ENV: &str = "MCPHOST_URL";
/// Set to `1` by the contract test: the proof prints `plan: ...` and exits 0
/// before any network call.
pub const DRY_ENV: &str = "MCPHOST_PROVE_DRY";
/// argv shape of the spawn, `$0` being the proof script and `$1` the endpoint.
pub const ARGV_SHAPE: &str = "bash <proof> <url>";

/// What `mcphost examples prove --list-contract` prints.
pub fn contract_text() -> String {
    format!(
        "argv: {ARGV_SHAPE}\n\
         env {URL_ENV}: the endpoint, same value as $1\n\
         env {DRY_ENV}: when set, print a line starting `plan:` and exit 0 without network\n\
         env {RUNNER_ENV}: host name recorded on the result row (default: hostname)\n"
    )
}

fn find_example(name: &str) -> Option<&'static Example> {
    EXAMPLES.iter().find(|e| e.name == name)
}

fn not_found(name: &str) -> AppError {
    AppError::Structured {
        code: EXAMPLE_NOT_FOUND,
        message: format!("no example named `{name}`; call host.example.list for the names"),
        data: json!({"name": name}),
    }
}

// ------------------------------------------------------------- host side

/// `admin.example_proof_report {name, ok, duration_s?, log_excerpt?, runner?, at?}`
/// (R2): upserts the verdict row. A failing report hides the example and
/// starts `red_since` when it was null; a passing one clears both.
pub async fn report(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".to_string()))?
        .to_string();
    let ok = args
        .get("ok")
        .and_then(Value::as_bool)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'ok' (boolean)".to_string()))?;
    if find_example(&name).is_none() {
        return Err(not_found(&name));
    }
    let now = now_unix();
    let at = args.get("at").and_then(Value::as_i64).unwrap_or(now);
    let previous = state.db.get_example_proof(name.clone()).await?;
    let prev_red_since = previous.as_ref().and_then(|p| p.red_since);
    let (hidden_reason, red_since) = if ok {
        (None, None)
    } else {
        (
            Some(format!("proof failed {}", &rfc3339_from_unix(at)[..10])),
            Some(prev_red_since.unwrap_or(at)),
        )
    };
    let row = ExampleProofRow {
        name: name.clone(),
        ok,
        at,
        duration_s: args.get("duration_s").and_then(Value::as_f64).unwrap_or(0.0),
        log_excerpt: args.get("log_excerpt").and_then(Value::as_str).unwrap_or_default().to_string(),
        runner: args.get("runner").and_then(Value::as_str).unwrap_or_default().to_string(),
        hidden_reason,
        red_since,
    };
    state.db.upsert_example_proof(row.clone()).await?;
    track_incident(state, &row).await?;
    Ok(proof_json(&row))
}

/// Component name the statusfeed incident is opened on (R4).
pub const INCIDENT_COMPONENT: &str = "examples";

fn incident_title(name: &str) -> String {
    format!("example {name} proof failed")
}

/// The open incident this mechanism holds for `name`, if any.
async fn open_incident_for(state: &AppState, name: &str) -> Result<Option<crate::db::Incident>, AppError> {
    let title = incident_title(name);
    Ok(state.db.list_open_incidents().await?.into_iter().find(|i| {
        i.title == title
            && serde_json::from_str::<Vec<String>>(&i.components_json).is_ok_and(|c| c.iter().any(|n| n == INCIDENT_COMPONENT))
    }))
}

/// R4: the first red of a streak opens one incident with the excerpt, each
/// further red appends to it, the green flip resolves it.
async fn track_incident(state: &AppState, row: &ExampleProofRow) -> Result<(), AppError> {
    let now = now_unix();
    let open = open_incident_for(state, &row.name).await?;
    match (row.ok, open) {
        (false, None) => {
            let title = incident_title(&row.name);
            let timeline = json!([{"ts": now, "type": "opened", "message": format!("{title}\n{}", row.log_excerpt)}]).to_string();
            state
                .db
                .insert_incident(title, "partial".to_string(), json!([INCIDENT_COMPONENT]).to_string(), now, timeline, true)
                .await?;
        }
        (false, Some(incident)) => {
            let message = format!("proof failed again\n{}", row.log_excerpt);
            state.db.incident_append_timeline(incident.id, json!({"ts": now, "type": "update", "message": message})).await?;
        }
        (true, Some(incident)) => {
            let entry = json!({"ts": now, "type": "closed", "message": "proof passed; example listed again"});
            state.db.incident_close(incident.id, entry, now).await?;
        }
        (true, None) => {}
    }
    Ok(())
}

pub fn proof_json(r: &ExampleProofRow) -> Value {
    json!({
        "name": r.name,
        "ok": r.ok,
        "at": r.at,
        "duration_s": r.duration_s,
        "log_excerpt": r.log_excerpt,
        "runner": r.runner,
        "hidden_reason": r.hidden_reason,
        "red_since": r.red_since,
    })
}

/// name -> (hidden_reason, red_since) for every currently hidden example.
async fn hidden_map(state: &AppState) -> Result<HashMap<String, (String, Option<i64>)>, AppError> {
    Ok(state
        .db
        .list_example_proofs()
        .await?
        .into_iter()
        .filter_map(|r| r.hidden_reason.map(|h| (r.name, (h, r.red_since))))
        .collect())
}

/// R5: the `examples` key of `/status.json`. Counts and names only; the
/// log excerpt stays on the admin side.
pub async fn status_block(state: &AppState) -> Result<Value, AppError> {
    let rows = state.db.list_example_proofs().await?;
    let hidden: Vec<Value> = rows
        .iter()
        .filter(|r| r.hidden_reason.is_some() && find_example(&r.name).is_some())
        .map(|r| json!({"name": r.name, "red_since": r.red_since, "runner": r.runner}))
        .collect();
    let total = EXAMPLES.len();
    Ok(json!({
        "total": total,
        "visible": total - hidden.len(),
        "hidden": hidden,
        "last_run": rows.iter().map(|r| r.at).max(),
    }))
}

/// `host.example.list {use_case?, client?}` (R3): every embedded example
/// whose proof has not failed, with the call that forks it.
pub async fn list(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let hidden = hidden_map(state).await?;
    let use_case = args.get("use_case").and_then(Value::as_str);
    let client = args.get("client").and_then(Value::as_str);
    let rows: Vec<Value> = EXAMPLES
        .iter()
        .filter(|e| !hidden.contains_key(e.name))
        .filter(|e| use_case.is_none_or(|u| e.use_case.contains(&u)))
        .filter(|e| client.is_none_or(|c| e.clients.contains(&c) || e.clients.contains(&"any")))
        .map(|e| {
            json!({
                "name": e.name,
                "title": e.title,
                "summary": e.summary,
                "use_case": e.use_case,
                "clients": e.clients,
                "tools": e.tools.iter().map(|t| t.name).collect::<Vec<_>>(),
                "secrets_needed": e.secrets_needed,
                "since": e.since,
                "fork": format!("host.example.fork {{\"name\": \"{}\"}}", e.name),
            })
        })
        .collect();
    Ok(json!({"count": rows.len(), "examples": rows}))
}

/// `host.example.fork {name}` (R3): a hidden example is refused with
/// `example_hidden {reason, red_since}` and nothing is written.
pub async fn fork(state: &AppState, args: &Value) -> Result<Value, AppError> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidArgs("missing required argument 'name'".to_string()))?;
    let Some(example) = find_example(name) else {
        return Err(not_found(name));
    };
    if let Some((reason, red_since)) = hidden_map(state).await?.remove(name) {
        return Err(AppError::Structured {
            code: EXAMPLE_HIDDEN,
            message: format!("example `{name}` is hidden: {reason}; pick another from host.example.list"),
            data: json!({"name": name, "reason": reason, "red_since": red_since}),
        });
    }
    let args_value: Value = serde_json::from_str(example.first_call.args_json).unwrap_or(Value::Null);
    Ok(json!({
        "name": example.name,
        "forked": [],
        "tools": example.tools.iter().map(|t| json!({"name": t.name, "kind": t.kind, "source": t.source})).collect::<Vec<_>>(),
        "secrets_needed": example.secrets_needed,
        "first_call": {"tool": example.first_call.tool, "args": args_value, "expect": example.first_call.expect},
        "next": "publish each tool with host.tool_publish, set the secrets, then run first_call",
    }))
}

// ---------------------------------------------------------------- runner

/// Everything [`run`] needs.
pub struct RunOpts {
    pub url: String,
    pub only: Option<String>,
    pub report: bool,
    /// R8: print the names that would run and execute nothing.
    pub dry_run: bool,
    pub admin_key: Option<String>,
    /// When set, an example's files are read from `<dir>/<name>/` instead
    /// of the embedded copy (a fixture host with a stub proof, in tests).
    pub files_dir: Option<PathBuf>,
}

/// One executed proof.
#[derive(Debug, Clone)]
pub struct ProofRun {
    pub name: String,
    pub ok: bool,
    pub duration_s: f64,
    pub log_excerpt: String,
}

/// The last `n` lines of `text`.
pub fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Write the example's files (embedded, or from `files_dir`) into `dest`.
fn extract(example: &Example, files_dir: Option<&Path>, dest: &Path) -> Result<(), String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    match files_dir.map(|d| d.join(example.name)).filter(|d| d.is_dir()) {
        Some(dir) => collect_dir(&dir, "", &mut files)?,
        None => files.extend(example.files.iter().map(|(p, b)| (p.to_string(), b.to_vec()))),
    }
    for (rel, bytes) in files {
        let target = dest.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        std::fs::write(&target, bytes).map_err(|e| format!("write {}: {e}", target.display()))?;
    }
    Ok(())
}

fn collect_dir(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?.flatten() {
        let rel = format!("{prefix}{}", entry.file_name().to_string_lossy());
        if entry.path().is_dir() {
            collect_dir(&entry.path(), &format!("{rel}/"), out)?;
        } else {
            let bytes = std::fs::read(entry.path()).map_err(|e| format!("read {rel}: {e}"))?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

async fn run_proof(example: &Example, opts: &RunOpts) -> ProofRun {
    let started = Instant::now();
    let fail = |log: String| ProofRun {
        name: example.name.to_string(),
        ok: false,
        duration_s: started.elapsed().as_secs_f64(),
        log_excerpt: tail_lines(&log, EXCERPT_LINES),
    };
    let dir = std::env::temp_dir().join(format!("mcphost-prove-{}-{}-{}", std::process::id(), example.name, now_unix()));
    if let Err(e) = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| extract(example, opts.files_dir.as_deref(), &dir)) {
        let _ = std::fs::remove_dir_all(&dir);
        return fail(format!("extract failed: {e}"));
    }
    let child = tokio::process::Command::new("bash")
        .arg(example.proof)
        .arg(&opts.url)
        .current_dir(&dir)
        .env(URL_ENV, &opts.url)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let result = tokio::time::timeout(PROOF_TIMEOUT, child).await;
    let _ = std::fs::remove_dir_all(&dir);
    match result {
        Err(_) => fail(format!("timed out after {}s", PROOF_TIMEOUT.as_secs())),
        Ok(Err(e)) => fail(format!("spawn failed: {e}")),
        Ok(Ok(out)) => {
            let mut log = String::from_utf8_lossy(&out.stdout).into_owned();
            log.push_str(&String::from_utf8_lossy(&out.stderr));
            ProofRun {
                name: example.name.to_string(),
                ok: out.status.success(),
                duration_s: started.elapsed().as_secs_f64(),
                log_excerpt: tail_lines(&log, EXCERPT_LINES),
            }
        }
    }
}

/// Env var that overrides the host name recorded on a result row.
pub const RUNNER_ENV: &str = "MCPHOST_PROVE_RUNNER";

/// The host this run executes on: `$MCPHOST_PROVE_RUNNER`, else the machine's
/// hostname, else `unknown`.
pub fn runner_name() -> String {
    let from_env = std::env::var(RUNNER_ENV).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let from_file = || std::fs::read_to_string("/etc/hostname").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let from_cmd = || {
        let out = std::process::Command::new("hostname").output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|v| !v.is_empty())
    };
    from_env.or_else(from_file).or_else(from_cmd).unwrap_or_else(|| "unknown".to_string())
}

async fn post_report(client: &reqwest::Client, opts: &RunOpts, run: &ProofRun) -> Result<(), String> {
    let key = opts.admin_key.as_deref().ok_or("MCPHOST_ADMIN_KEY is unset")?;
    let params = json!({"name": "admin.example_proof_report", "arguments": {
        "name": run.name, "ok": run.ok, "duration_s": run.duration_s,
        "log_excerpt": run.log_excerpt, "runner": runner_name(), "at": now_unix(),
    }});
    let req = RpcRequest { method: "tools/call", params, id: 1, bearer: Some(key) };
    let result = rpc_call(client, opts.url.trim_end_matches('/'), req).await.map_err(|e| e.to_string())?;
    if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("admin.example_proof_report refused: {}", extract_structured(&result)));
    }
    Ok(())
}

/// R1: run every embedded example's proof (or `only`), print one line per
/// example to `out`, post each verdict when `report`. Returns the runs and
/// whether every proof passed and every report was accepted.
pub async fn run(opts: &RunOpts, out: &mut dyn Write) -> Result<(Vec<ProofRun>, bool), String> {
    let selected: Vec<&Example> = match &opts.only {
        Some(n) => vec![find_example(n).ok_or_else(|| format!("no example named `{n}`"))?],
        None => EXAMPLES.iter().collect(),
    };
    if opts.dry_run {
        for example in &selected {
            let _ = writeln!(out, "{}", example.name);
        }
        return Ok((Vec::new(), true));
    }
    let client = reqwest::Client::new();
    let mut runs = Vec::new();
    let mut all_ok = true;
    for example in selected {
        let proof = run_proof(example, opts).await;
        let _ = writeln!(out, "{} {} {:.1}s", proof.name, if proof.ok { "PASS" } else { "FAIL" }, proof.duration_s);
        if !proof.ok {
            for line in proof.log_excerpt.lines() {
                let _ = writeln!(out, "  {line}");
            }
        }
        all_ok &= proof.ok;
        if opts.report
            && let Err(e) = post_report(&client, opts, &proof).await
        {
            let _ = writeln!(out, "{} report failed: {e}", proof.name);
            all_ok = false;
        }
        runs.push(proof);
    }
    Ok((runs, all_ok))
}

crate::err_code! {
    pub EXAMPLE_HIDDEN,
    code: "example_hidden", family: Share, class: HostRefusal, retryable: true,
    remedy: "The example failed its nightly proof and is hidden (data.reason, data.red_since); pick another from host.example.list or retry after it is green again.",
}

crate::err_code! {
    pub EXAMPLE_NOT_FOUND,
    code: "example_not_found", family: Share, class: AgentFault, retryable: false,
    remedy: "Call host.example.list for the example names and pass one of them as name.",
}

