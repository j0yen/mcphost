//! PRD-mcphost-push-from-directory: `mcphost push <dir> --url <tenant url>`
//! packs a tool directory ([`crate::tooldir::pack`]), publishes it once and
//! prints what happened. A push whose packed spec hashes to the host's
//! current `source_sha256` ([`crate::tooldir::content_hash`], the function
//! the server itself stores) is `unchanged` and writes no version row.
//!
//! The tenant URL is a credential: it is read from `--url` / `$MCPHOST_URL`
//! and never printed.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::client::{BAD_RESPONSE, RpcFailure, RpcRequest, extract_structured, rpc_call};

/// Exit codes (PRD R5).
pub const EXIT_OK: i32 = 0;
pub const EXIT_TESTS_RED: i32 = 1;
pub const EXIT_REFUSED: i32 = 2;
pub const EXIT_NETWORK: i32 = 3;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default)]
pub struct Opts {
    pub dir: PathBuf,
    pub url: Option<String>,
    pub json: bool,
    pub signup: bool,
    pub watch: bool,
}

/// One finished push, ready to print.
#[derive(Debug, Clone)]
pub struct Report {
    pub exit: i32,
    pub value: Value,
    pub text: String,
}

/// Why a host call did not yield a result.
#[derive(Debug)]
enum Failure {
    /// The host answered and said no: its own `code` and `message`.
    Refused { code: String, message: String },
    /// The host could not be reached, or answered with a 5xx.
    Network(String),
}

impl Failure {
    fn report(self) -> Report {
        match self {
            Failure::Refused { code, message } => Report {
                exit: EXIT_REFUSED,
                value: json!({"ok": false, "code": code, "message": message}),
                text: format!("refused {code}: {message}"),
            },
            Failure::Network(detail) => Report {
                exit: EXIT_NETWORK,
                value: json!({"ok": false, "code": "host_unreachable", "message": detail}),
                text: format!("network: {detail}"),
            },
        }
    }
}

fn failure_from(f: &RpcFailure) -> Failure {
    match f {
        RpcFailure::Unreachable(detail) => Failure::Network(detail.clone()),
        RpcFailure::Unparsed { status, detail, .. } if *status >= 500 => Failure::Network(detail.clone()),
        RpcFailure::Unparsed { detail, .. } => Failure::Refused { code: BAD_RESPONSE.to_string(), message: detail.clone() },
        RpcFailure::JsonRpc { status, error, .. } => {
            let code = error
                .pointer("/data/error_code")
                .and_then(Value::as_str)
                .unwrap_or("jsonrpc_error")
                .to_string();
            let message = error.get("message").and_then(Value::as_str).unwrap_or_default().to_string();
            if *status >= 500 && code == "jsonrpc_error" {
                Failure::Network(message)
            } else {
                Failure::Refused { code, message }
            }
        }
    }
}

/// The `/mcp` endpoint's base: `https://h/u/<secret>/mcp` -> `https://h/u/<secret>`
/// ([`rpc_call`] appends `/mcp`).
pub fn mcp_base(url: &str) -> String {
    let t = url.trim().trim_end_matches('/');
    t.strip_suffix("/mcp").unwrap_or(t).to_string()
}

struct Host {
    client: reqwest::Client,
    base: String,
}

impl Host {
    fn new(url: &str) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self { client, base: mcp_base(url) }
    }

    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, Failure> {
        rpc_call(
            &self.client,
            &self.base,
            RpcRequest { method: "tools/call", params: json!({"name": tool, "arguments": arguments}), id: 1, bearer: None },
        )
        .await
        .map(|r| extract_structured(&r))
        .map_err(|f| self.redact(failure_from(&f)))
    }

    /// A transport error names the URL it failed on, and the URL is the
    /// credential: it never reaches a report.
    fn redact(&self, f: Failure) -> Failure {
        let scrub = |s: String| s.replace(&self.base, "<tenant url>");
        match f {
            Failure::Network(d) => Failure::Network(scrub(d)),
            Failure::Refused { code, message } => Failure::Refused { code, message: scrub(message) },
        }
    }
}

/// `--url`, then `$MCPHOST_URL`, then the file `--signup` wrote.
fn resolve_url(opts: &Opts) -> Option<String> {
    opts.url
        .clone()
        .or_else(|| std::env::var("MCPHOST_URL").ok())
        .or_else(|| std::fs::read_to_string(config_path()?).ok().map(|t| t.trim().to_string()))
        .filter(|u| !u.trim().is_empty())
}

/// `$XDG_CONFIG_HOME/mcphost/tenant.url` (else `~/.config/...`).
pub fn config_path() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config").map(|d| d.join("mcphost/tenant.url"))
}

fn xdg_dir(var: &str, home_rel: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(home_rel)))
}

/// Writes the tenant URL (a credential) to `path`, mode 0600.
fn write_secret_file(path: &std::path::Path, secret: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(secret.as_bytes())?;
    file.write_all(b"\n")
}

/// `--signup`: an anonymous `host.whoami` implicitly creates a tenant and
/// answers with its `onboarding` block. The URL goes to the 0600 config
/// file; the caller gets the URL to keep pushing with, plus the tenant id
/// and claim link (the only parts that may be printed).
async fn signup(opts: &Opts) -> Result<(String, Value), Report> {
    let host_url = opts
        .url
        .clone()
        .or_else(|| std::env::var("MCPHOST_URL").ok())
        .filter(|u| !u.trim().is_empty())
        .map_or_else(|| "https://mcphost.dev".to_string(), |u| origin(&u));
    let onboarding = Host::new(&host_url).call("host.whoami", json!({})).await.map_err(Failure::report)?["onboarding"].clone();
    let url = onboarding["url"].as_str().map(str::to_string);
    let Some(url) = url else {
        return Err(local_failure("signup: the host returned no tenant url".into()));
    };
    let path = config_path().ok_or_else(|| local_failure("signup: no $XDG_CONFIG_HOME or $HOME to write the tenant url to".into()))?;
    write_secret_file(&path, &url).map_err(|e| local_failure(format!("signup: cannot write {}: {e}", path.display())))?;
    let who = json!({"tenant": onboarding["tenant"], "claim_url": onboarding["claim_url"]});
    Ok((url, who))
}

/// One push of `opts.dir`.
pub async fn push_once(opts: &Opts) -> Report {
    let (url, who) = if opts.signup {
        match signup(opts).await {
            Ok((url, who)) => (Some(url), Some(who)),
            Err(r) => return r,
        }
    } else {
        (resolve_url(opts), None)
    };
    let mut report = match push_inner(opts, url).await {
        Ok(r) | Err(r) => r,
    };
    if let Some(who) = who {
        let line = format!("tenant {} · claim: {}", who["tenant"].as_str().unwrap_or_default(), who["claim_url"].as_str().unwrap_or("none"));
        report.text = format!("{line}\n{}", report.text);
        if let (Some(obj), Some(w)) = (report.value.as_object_mut(), who.as_object()) {
            obj.extend(w.clone());
        }
    }
    report
}

fn local_failure(message: String) -> Report {
    Report { exit: EXIT_REFUSED, value: json!({"ok": false, "code": "invalid_spec", "message": message}), text: format!("refused invalid_spec: {message}") }
}

async fn push_inner(opts: &Opts, url: Option<String>) -> Result<Report, Report> {
    let url = url.ok_or_else(|| local_failure("no tenant url: pass --url or set $MCPHOST_URL".into()))?;
    let payload = crate::tooldir::pack(&opts.dir).map_err(|e| local_failure(e.message))?;
    let host = Host::new(&url);
    let tool = payload["name"].as_str().unwrap_or_default().to_string();
    let sha = crate::tooldir::content_hash(&payload["spec"]);

    if let Some(version) = current_version_if_same(&host, &tool, &sha).await? {
        return Ok(Report {
            exit: EXIT_OK,
            value: json!({"ok": true, "tool": tool, "version": version, "unchanged": true, "source_sha256": sha}),
            text: format!("unchanged (source_sha256 {sha})"),
        });
    }

    let tests = run_tests(&host, &opts.dir, &payload).await?;
    if tests.ok < tests.total {
        let mut text = format!("tests {}/{}", tests.ok, tests.total);
        for line in &tests.failures {
            text.push_str(&format!("\n  {line}"));
        }
        return Ok(Report {
            exit: EXIT_TESTS_RED,
            value: json!({"ok": false, "tool": tool, "tests": tests.to_json()}),
            text,
        });
    }

    let published = host.call("host.tool.publish", payload).await.map_err(Failure::report)?;
    let version = published["version"].clone();
    record_push(&sha, &tool, &version);
    let call_line = format!("host.tool_call(name={}, args={})", json!(tool), tests.first_args);
    let proof = proof_url(&host, &url, published["name"].as_str().unwrap_or(&tool)).await?;
    Ok(Report {
        exit: EXIT_OK,
        value: json!({
            "ok": true, "tool": tool, "version": version, "unchanged": false, "source_sha256": sha,
            "tests": tests.to_json(), "call_line": call_line, "proof": proof,
        }),
        text: format!("{} · published {tool} v{version} · call: {call_line} · proof: {proof}", tests.summary()),
    })
}

/// `$XDG_STATE_HOME/mcphost/pushes.jsonl` (else `~/.local/state/...`).
pub fn pushes_path() -> Option<PathBuf> {
    xdg_dir("XDG_STATE_HOME", ".local/state").map(|d| d.join("mcphost/pushes.jsonl"))
}

/// Appends `{sha256, tool, version, ts}` after a publish succeeded -- the
/// only writer, so a failed or unreachable push leaves no row behind. A
/// record that cannot be written never fails the push itself.
fn record_push(sha: &str, tool: &str, version: &Value) {
    use std::io::Write;
    let Some(path) = pushes_path() else { return };
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let row = json!({"sha256": sha, "tool": tool, "version": version, "ts": ts});
    let _ = path.parent().map(std::fs::create_dir_all);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{row}");
    }
}

/// The `/d/<ns>/<tool>` demo link when the host lists `host.demo_url`, else `none`.
async fn proof_url(host: &Host, url: &str, qualified: &str) -> Result<String, Report> {
    let listed = rpc_call(
        &host.client,
        &host.base,
        RpcRequest { method: "tools/list", params: json!({}), id: 1, bearer: None },
    )
    .await
    .map_err(|f| host.redact(failure_from(&f)).report())?;
    let offered = listed["tools"]
        .as_array()
        .is_some_and(|ts| ts.iter().any(|t| matches!(t["name"].as_str(), Some("host.demo_url" | "host.demo.url"))));
    let (Some((ns, name)), true) = (qualified.split_once('.'), offered) else {
        return Ok("none".into());
    };
    Ok(format!("{}/d/{ns}/{name}", origin(url)))
}

/// `https://h:8080` of `https://h:8080/u/<secret>/mcp`.
fn origin(url: &str) -> String {
    let u = url.trim();
    let after = u.find("://").map_or(0, |i| i + 3);
    u[after..].find('/').map_or(u.to_string(), |i| u[..after + i].to_string())
}

/// The tally of a directory's `tests/*.json` run through `host.spec.test`.
struct Tally {
    ok: usize,
    total: usize,
    failures: Vec<String>,
    /// The first test's args: what the copy-paste call line carries.
    first_args: Value,
}

impl Tally {
    fn summary(&self) -> String {
        if self.total == 0 { "tests none".into() } else { format!("tests {}/{} ok", self.ok, self.total) }
    }

    fn to_json(&self) -> Value {
        json!({"ok": self.ok, "total": self.total, "failures": self.failures})
    }
}

/// Most invocations one `host.spec.test` call accepts (`MAX_TEST_INVOCATIONS`).
const SPEC_TEST_BATCH: usize = crate::kinds::MAX_TEST_INVOCATIONS;

/// Runs every declared test through the host's `host.spec.test`; a red
/// invocation is one line naming the test file, what was expected and the
/// host's own `code` and message.
async fn run_tests(host: &Host, dir: &std::path::Path, payload: &Value) -> Result<Tally, Report> {
    let mut cases: Vec<(String, Value, Option<Value>)> = Vec::new();
    let mut failures = Vec::new();
    for path in crate::tooldir::test_files(dir) {
        let label = format!("tests/{}", path.file_name().and_then(|n| n.to_str()).unwrap_or_default());
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()));
        match parsed {
            Ok(v) => cases.push((label, v.get("args").cloned().unwrap_or_else(|| json!({})), v.get("expect").cloned())),
            Err(e) => failures.push(format!("{label}: not a readable test file ({e})")),
        }
    }
    let total = cases.len() + failures.len();
    let first_args = cases.first().map_or_else(|| json!({}), |(_, args, _)| args.clone());
    let mut ok = 0;
    for batch in cases.chunks(SPEC_TEST_BATCH) {
        let invocations: Vec<Value> = batch.iter().map(|(_, args, _)| args.clone()).collect();
        let report = host
            .call("host.spec.test", json!({"kind": payload["kind"], "spec": payload["spec"], "invocations": invocations}))
            .await
            .map_err(Failure::report)?;
        let results = report["invocations"].as_array().cloned().unwrap_or_default();
        for (i, (label, _, expect)) in batch.iter().enumerate() {
            let result = results.get(i).cloned().unwrap_or(Value::Null);
            // A kind wraps the tool's return value as `output.result`.
            let got = result["output"].get("result").unwrap_or(&result["output"]);
            match (result["ok"] == json!(true), expect) {
                (true, Some(want)) if got != want => {
                    failures.push(format!("{label}: expected {want}, got {got}"));
                }
                (true, _) => ok += 1,
                (false, expect) => failures.push(format!("{label}: {}{}", expected_prefix(expect), describe_failure(&result))),
            }
        }
    }
    Ok(Tally { ok, total, failures, first_args })
}

fn expected_prefix(expect: &Option<Value>) -> String {
    expect.as_ref().map(|w| format!("expected {w}, got ")).unwrap_or_default()
}

/// `tool_exception KeyError: 'name'`: the host's `code`, then its exception
/// class and message when it sent them.
fn describe_failure(result: &Value) -> String {
    let code = result["code"].as_str().unwrap_or("unknown_error");
    let class = result["exception_class"].as_str().map(|c| format!(" {c}:")).unwrap_or_else(|| ":".into());
    format!("{code}{class} {}", result["message"].as_str().unwrap_or_default())
}

/// `Some(version)` when the host's current version of `tool` already carries
/// `sha`; `None` for an unknown tool or a different hash.
async fn current_version_if_same(host: &Host, tool: &str, sha: &str) -> Result<Option<Value>, Report> {
    match host.call("host.tool.history", json!({"name": tool})).await {
        Ok(history) => Ok(history["versions"]
            .as_array()
            .and_then(|vs| vs.iter().find(|v| v["current"] == json!(true)))
            .filter(|v| v["source_sha256"].as_str() == Some(sha))
            .map(|v| v["version"].clone())),
        Err(Failure::Refused { code, .. }) if code == "tool_not_found" => Ok(None),
        Err(f) => Err(f.report()),
    }
}

/// Prints `report` (JSON line with `json`, else its text) and returns its exit code.
pub fn emit(report: &Report, json: bool) -> i32 {
    if json {
        println!("{}", report.value);
    } else {
        println!("{}", report.text);
    }
    report.exit
}

/// How often `--watch` looks at the directory, and how long it waits for the
/// saves to stop before pushing (PRD R6's debounce, kept well under 2 s).
const WATCH_POLL: Duration = Duration::from_millis(100);
const WATCH_DEBOUNCE: Duration = Duration::from_millis(250);

/// A hash of every file's path and bytes under `dir`: a save that rewrites
/// identical bytes (new mtime, same content) leaves it unchanged.
fn fingerprint(dir: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    let mut h = Sha256::new();
    for f in files {
        h.update(f.to_string_lossy().as_bytes());
        h.update(std::fs::read(&f).unwrap_or_default());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// `--watch`: push, then push again whenever the directory's content changes
/// (after [`WATCH_DEBOUNCE`] of quiet). An `unchanged` push prints nothing.
/// Runs until interrupted.
pub async fn watch(opts: &Opts) -> i32 {
    let mut seen = String::new();
    loop {
        let mut now = fingerprint(&opts.dir);
        if now != seen {
            // Debounce: keep waiting while saves are still landing.
            loop {
                tokio::time::sleep(WATCH_DEBOUNCE).await;
                let again = fingerprint(&opts.dir);
                if again == now {
                    break;
                }
                now = again;
            }
            seen = now;
            let report = push_once(opts).await;
            if report.value["unchanged"] != json!(true) {
                emit(&report, opts.json);
            }
        }
        tokio::time::sleep(WATCH_POLL).await;
    }
}
