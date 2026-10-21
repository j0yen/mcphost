//! Shared integration-test harness: spin up a real `mcphost` server on an
//! ephemeral port against a fresh temp `$MCPHOST_DATA_DIR`, and a tiny
//! JSON-RPC-over-streamable-HTTP client to drive it.
//!
//! This module is `mod`-included separately into every `tests/ac*.rs`
//! binary (the standard Rust integration-test pattern), so any one binary
//! uses only a subset of it — `dead_code` is expected, not a real
//! findings.
#![allow(dead_code)]

pub mod testoauth;

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mcphost::db::Db;
use mcphost::kinds::KindRegistry;
use mcphost::kinds::http::{HttpKind, LookupFuture, NameLookup};
use mcphost::kinds::python::PythonKind;
use mcphost::registry::RegistryConfig;
use mcphost::secrets::SecretBox;
use mcphost::state::AppState;
use serde_json::{Value, json};

/// Path a test should `Command::new` when it re-executes ITS OWN binary.
/// On Linux `/proc/self/exe` execs the running inode even if the path in
/// `target/` was unlinked and re-created underneath the suite (a nested
/// `cargo` relink, the VFX flake class); elsewhere the plain
/// `current_exe()` path is all there is.
pub fn self_exe() -> PathBuf {
    if cfg!(target_os = "linux") {
        PathBuf::from("/proc/self/exe")
    } else {
        std::env::current_exe().expect("this test binary's own path")
    }
}

/// Env names cargo INJECTS into the process of every test it runs (not user
/// config such as `CARGO_HOME` / `CARGO_TARGET_DIR`).
fn is_cargo_injected_env(name: &str) -> bool {
    ["CARGO_PKG_", "CARGO_MANIFEST_", "CARGO_BIN_EXE_"].iter().any(|p| name.starts_with(p))
        || ["CARGO_BIN_NAME", "CARGO_CRATE_NAME", "CARGO_PRIMARY_PACKAGE", "CARGO_TARGET_TMPDIR", "OUT_DIR"].contains(&name)
}

/// Call on every `Command` a test uses to run `cargo` (or a script that runs
/// it) in the SAME target dir as the running suite. A third-party build
/// script that tracks one of the injected names (`ring` tracks
/// `CARGO_MANIFEST_DIR` and `OUT_DIR`) sees it "changed" from the outer build's unset to
/// the nested cargo's inherited value, reruns, and rebuilds `mcphost` plus
/// relinks `deps/suite_*` and `target/debug/mcphost` under the running suite
/// (the VFX flake class, second cause).
pub fn strip_cargo_injected_env(cmd: &mut std::process::Command) -> &mut std::process::Command {
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(is_cargo_injected_env) {
            cmd.env_remove(k);
        }
    }
    cmd
}

/// Serializes the tests that read or rewrite the doc files
/// `scripts/sandbox-api-doc-check.sh` owns: `--write` rewrites them in place
/// and a concurrent copy or check can see a half-written file.
pub static DOC_CHECK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A scratch copy of every file `scripts/sandbox-api-doc-check.sh` reads or
/// regenerates, laid out at the same relative paths. A test that needs to run
/// the script in `--write` mode does it here, never on the repo's own tree: a
/// `--write` on the real tree is visible, half-finished, to every concurrently
/// running test that reads `www/llms.txt`, `README.md` and the other docs.
pub fn doc_check_scratch(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::temp_dir().join(format!(
        "mcphost-doccheck-{label}-{}-{}",
        process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _guard = DOC_CHECK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for rel in [
        "scripts/sandbox-api-doc-check.sh",
        "scripts/host-tools-doc-check.sh",
        "scripts/protocol-frames-check.sh",
        "src/protocol/frames.rs",
        "docs/protocol/frames.md",
        "www/frames.json",
        "src/write_verbs.rs",
        "src/kinds/python.rs",
        "src/oauth.rs",
        "docs/host-tools.md",
        "docs/kinds/python.md",
        "docs/kinds/js.md",
        "docs/oauth-connect.md",
        "www/llms.txt",
        "docs/protocol/capabilities.md",
        "docs/protocol/prompts.md",
        // seed SDB: the bridge-modules block in docs/sandbox-api.md is a --write output.
        "docs/sandbox-api.md",
        "src/prompts.rs",
        "www/llms-install.md",
        "www/llms-full.txt",
        "www/index.html",
        "plugin/skills/mcphost/SKILL.md",
        "README.md",
        "llms-install.md",
    ] {
        let dest = dir.join(rel);
        std::fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir scratch");
        std::fs::copy(root.join(rel), &dest).unwrap_or_else(|e| panic!("copy {rel}: {e}"));
    }
    dir
}

/// The anonymous `/healthz` key set, read from the crate const so no test
/// restates it (`scripts/healthz-keys-lint.py` fails a literal).
pub fn anon_healthz_keys() -> &'static [&'static str] {
    mcphost::ANON_HEALTHZ_KEYS
}

/// Asserts an anonymous `/healthz` body has exactly the keys of
/// `anon_healthz_keys()` and the expected `ok` / build-identity values.
pub fn assert_anon_healthz(body: &Value, ok: bool) {
    let mut keys: Vec<&str> = body.as_object().expect("anonymous /healthz body is an object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, anon_healthz_keys(), "anonymous /healthz keys: {body}");
    assert_eq!(body["ok"], json!(ok), "anonymous /healthz ok: {body}");
    assert_eq!(body["version"], json!(mcphost::build_info::BUILD.version), "anonymous /healthz version: {body}");
    assert_eq!(body["git_sha"], json!(mcphost::build_info::BUILD.git_sha), "anonymous /healthz git_sha: {body}");
}

/// PRD-mcphost-contract-output-schemas R4: every successful host-tool
/// `structuredContent` this harness sees is appended to
/// `target/outsch/<canonical tool>.jsonl`, one row per line. A wire name that
/// is an alias or a flattened form is recorded under its canonical. Tenant
/// tools, error results and results without `structuredContent` are skipped.
/// Best effort: a failed write never fails the calling test.
pub fn outsch_capture(wire_name: &str, result: &Value) {
    use std::io::Write;
    use std::sync::OnceLock;
    static CANONICALS: OnceLock<HashMap<String, String>> = OnceLock::new();
    let map = CANONICALS.get_or_init(|| {
        let kinds = KindRegistry::with_builtin();
        let mut m = HashMap::new();
        for c in mcphost::handler::canonical_control_plane_names(&kinds) {
            m.insert(c.clone(), c.clone());
            if let Some(f) = mcphost::tool_aliases::flattened_form(&c) {
                m.insert(f, c.clone());
            }
            for a in mcphost::tool_aliases::aliases_of(&c) {
                m.insert(a.to_string(), c.clone());
            }
        }
        m
    });
    let Some(canonical) = map.get(wire_name) else { return };
    if result.get("isError").and_then(Value::as_bool).unwrap_or(false) {
        return;
    }
    let Some(structured) = result.get("structuredContent").filter(|v| v.is_object()) else {
        return;
    };
    // A forwarded call (`host.tool_call` / `host.tool_test` with a dotted verb,
    // PRD-mcphost-verb-forwarding) returns the FORWARDED verb's output plus
    // `client_tool` / `forwarded_to`: it is not the wrapper's own output, so
    // recording it under the wrapper's canonical name puts another verb's
    // fields (`rows`, `ok`, `next`, `welcome_back`, ...) into the wrapper's
    // corpus, and `oschv_ac04` then fails for whichever run saw those rows.
    if structured.get("forwarded_to").is_some() {
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/outsch");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(mut line) = serde_json::to_string(structured) else { return };
    line.push('\n');
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{canonical}.jsonl")))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Rows per tool: the committed corpus (`tests/fixtures/outsch/`, captured
/// from this suite) plus whatever this run's harness already recorded under
/// `target/outsch/`.
pub fn outsch_rows() -> std::collections::BTreeMap<String, Vec<Value>> {
    let mut out: std::collections::BTreeMap<String, Vec<Value>> = std::collections::BTreeMap::new();
    for dir in ["tests/fixtures/outsch", "target/outsch"] {
        let Ok(entries) = std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(tool) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let text = std::fs::read_to_string(&path).expect("read rows");
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                out.entry(tool.to_string())
                    .or_default()
                    .push(serde_json::from_str(line).unwrap_or_else(|e| panic!("{}: {e}", path.display())));
            }
        }
    }
    out
}

pub const ADMIN_KEY: &str = "test-admin-key-not-for-production";

/// A [`NameLookup`] with a fixed set of hostname -> address answers, so a
/// test can make a DNS name resolve to an arbitrary (including private)
/// address without depending on real network DNS (used by the AC9
/// DNS-rebinding test). A lookup miss is a resolution failure, matching a
/// real resolver's `NXDOMAIN` behavior.
pub struct FixedLookup(pub HashMap<String, Vec<IpAddr>>);

impl NameLookup for FixedLookup {
    fn lookup(&self, host: String) -> LookupFuture {
        let answer = self.0.get(&host).cloned();
        Box::pin(async move {
            answer.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("no fixture for {host}"),
                )
            })
        })
    }
}

/// `echo` (base) + a *relaxed* `http` kind: loopback and plain `http://`
/// both allowed, so a test can call a local `wiremock` stub upstream while
/// exercising the real template/redaction/rate-limit/error-code paths, not
/// a bypassed version of them. Used by every `http`-kind test except the
/// SSRF-specific ones (AC2, AC9), which need the production-strict policy
/// (see [`http_kind_registry_strict`]) to prove the block actually happens.
pub fn http_kind_registry() -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    let lookup: Arc<dyn NameLookup> = Arc::new(FixedLookup(HashMap::new()));
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    kinds
}

/// Same, but the rate limit is overridden to `limit_per_minute` (AC13
/// doesn't need 600 real HTTP calls to prove the limiter wires up end to
/// end through the real dispatch path).
pub fn http_kind_registry_with_rate_limit(limit_per_minute: u32) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    let lookup: Arc<dyn NameLookup> = Arc::new(FixedLookup(HashMap::new()));
    kinds.register(Arc::new(HttpKind::for_test_with_rate_limit(
        "127.0.0.1",
        lookup,
        limit_per_minute,
    )));
    kinds
}

/// `echo` (base) + the *production* (https-only, loopback-blocked) `http`
/// kind policy, with `lookup` standing in for real DNS. AC2 (publish-time)
/// and AC9 (call-time) both reject before any connection is attempted, so
/// neither needs a reachable upstream.
pub fn http_kind_registry_strict(lookup: FixedLookup) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(HttpKind::for_test_strict(
        "127.0.0.1",
        Arc::new(lookup),
    )));
    kinds
}

/// `echo` (base) + a `python` kind rooted at `data_dir` (the caller's own
/// [`TempDataDir`], kept alive for the test's duration -- `PythonKind` only
/// needs a writable directory for `envs/`/`scratch/`, independent of
/// whatever data dir `TestServer` builds its own SQLite database in). Used
/// by the `python`-kind AC test suite (`python_ac*.rs`).
pub fn python_kind_registry(data_dir: &Path) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(PythonKind::new(data_dir)));
    kinds
}

/// [`python_kind_registry`] with the python kind told the host's own domain
/// (PRD-mcphost-unknown-import-domain-hint R1) -- what `main.rs` does with
/// `own_domain_from_url(public_url)`.
pub fn python_kind_registry_with_domain(data_dir: &Path, own_domain: &str) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(PythonKind::new(data_dir).with_own_domain(own_domain)));
    kinds
}

/// PRD-mcphost-js-kind: `echo` (base) + a `js` kind whose scratch dirs live
/// under `data_dir` -- the js-kind AC suite's usual entry point.
pub fn js_kind_registry(data_dir: &Path) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(mcphost::kinds::js::JsKind::new(data_dir)));
    kinds
}

/// `echo` (base) + a test-relaxed `http` + a `python` kind rooted at
/// `data_dir` -- every kind `main.rs` registers in production, for tests
/// (PRD-mcphost-publish-first-try) that need to see the full
/// `host.tool_publish`/`host.quickstart` surface across every registered
/// kind, not just one.
pub fn all_kinds_registry(data_dir: &Path) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    let lookup: Arc<dyn NameLookup> = Arc::new(FixedLookup(HashMap::new()));
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    kinds.register(Arc::new(PythonKind::new(data_dir)));
    kinds
}

/// All five shipped kinds (`echo`, `http`, `python`, `chain`, `wasm`) --
/// PRD-mcphost-publish-schema-from-registry's `pubschema_*` tests need the
/// full production registry so the published schema covers every kind.
pub fn five_kinds_registry(data_dir: &Path) -> KindRegistry {
    let mut kinds = all_kinds_registry(data_dir);
    kinds.register(Arc::new(mcphost::kinds::chain::ChainKind));
    kinds.register(Arc::new(mcphost::kinds::wasm::WasmKind::new()));
    kinds
}

/// `echo` (base) + `chain` + a `python` kind rooted at `data_dir` --
/// PRD-mcphost-chain-host-steps AC5's own entry point: a tenant must be
/// able to publish the `host.quickstart kind=chain` example's first step
/// (the python starter, `control::STARTER_TOOL_NAME`), which needs the real
/// `python` kind registered, not just `chain_kind_registry()`'s `echo`.
pub fn chain_and_python_kind_registry(data_dir: &Path) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(mcphost::kinds::chain::ChainKind));
    kinds.register(Arc::new(PythonKind::new(data_dir)));
    kinds
}

/// `echo` (base) + `chain` -- the `compose_ac*.rs` suite's usual entry
/// point (PRD-mcphost-composition). `echo` is enough to compose against
/// directly (its `call` just validates and returns its args); a chain step
/// naming another `chain` tool composes against this same registry too,
/// since `chain`'s own step dispatch (`kinds::compose_call`) resolves
/// through whatever `KindRegistry` the calling `CallCtx` carries.
pub fn chain_kind_registry() -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(mcphost::kinds::chain::ChainKind));
    kinds
}

/// `echo` (base) + `chain` + a test-relaxed `http` -- PRD-mcphost-tool-
/// test-truth's `testtruth_ac0[1-3]*.rs` fixtures publish an `http` step
/// so they can declare `outputs` (checked by `kinds::chain::verdict`'s own
/// static mapping check) without needing a real sandbox; the dry run never
/// actually reaches the upstream, so its URL need not be reachable.
pub fn chain_and_http_kind_registry() -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(mcphost::kinds::chain::ChainKind));
    let lookup: Arc<dyn NameLookup> = Arc::new(FixedLookup(HashMap::new()));
    kinds.register(Arc::new(HttpKind::for_test("127.0.0.1", lookup)));
    kinds
}

/// PRD-mcphost-sandbox-ready: a shell script body for
/// `PythonKind::set_selftest_interpreter_for_test` that writes
/// `stderr_line` to stderr and exits 1 -- simulates one specific sandbox
/// failure mode (a broken interpreter, a namespace-setup failure) without
/// needing an actually-broken host. Reads and discards stdin first so
/// writing to it (the self-test probe always sends its trivial source on
/// stdin) never blocks on a full pipe.
pub fn fake_interpreter_failing(stderr_line: &str) -> String {
    format!(
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}' >&2\nexit 1\n",
        stderr_line.replace('\'', "'\\''")
    )
}

/// PRD-mcphost-sandbox-ready: a `python` kind whose sandbox self-test
/// interpreter and periodic-recheck interval are both test-controlled --
/// see `PythonKind::for_test_with_selftest`'s doc comment. The returned
/// `Arc<PythonKind>` lets a test call `set_selftest_interpreter_for_test`,
/// `run_startup_selftest`, `recheck_sandbox`, etc. on the concrete type
/// *before* it's registered as `Arc<dyn Kind>`, then hand the same
/// registration off to `TestServer`.
pub fn python_kind_registry_with_selftest(
    data_dir: &Path,
    recheck_secs: u64,
) -> (KindRegistry, Arc<PythonKind>) {
    let mut kinds = KindRegistry::with_builtin();
    let py = Arc::new(PythonKind::for_test_with_selftest(data_dir, recheck_secs));
    kinds.register(py.clone());
    (kinds, py)
}

/// AC13: a small concurrency limit so admission control can be proven
/// without 21 real sandboxed calls in flight.
pub fn python_kind_registry_with_concurrency(data_dir: &Path, limit: usize) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(PythonKind::for_test_with_concurrency(
        data_dir, limit,
    )));
    kinds
}

/// AC14: a tiny CPU budget so the rate limit can be proven without 600 real
/// CPU-seconds of calls.
pub fn python_kind_registry_with_cpu_budget_ms(data_dir: &Path, budget_ms: u64) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(PythonKind::for_test_with_cpu_budget_ms(
        data_dir, budget_ms,
    )));
    kinds
}

/// PRD-mcphost-code-tools-warm-pool AC1/AC2/AC5/AC8: a short TTL and small
/// per-tenant/box-wide bounds so the warm-pool test suite doesn't need a
/// real 60s wait or 16 real sandboxed tools to prove reaping/bounding.
pub fn python_kind_registry_with_warm_pool(
    data_dir: &Path,
    ttl: std::time::Duration,
    per_tenant: usize,
    max_total: usize,
) -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(PythonKind::for_test_with_warm_pool(
        data_dir, ttl, per_tenant, max_total,
    )));
    kinds
}

/// PRD-mcphost-wasm-kind: `echo` (base) + a `wasm` kind -- the wasm-kind AC
/// suite's usual entry point.
pub fn wasm_kind_registry() -> KindRegistry {
    let mut kinds = KindRegistry::with_builtin();
    kinds.register(Arc::new(mcphost::kinds::wasm::WasmKind::new()));
    kinds
}

/// Same as [`wasm_kind_registry`], but also hands back the concrete
/// `Arc<WasmKind>` (registered as itself, not yet erased to `Arc<dyn Kind>`)
/// so a test can read its cache counters (AC8) after driving calls through
/// the server built from the returned registry.
pub fn wasm_kind_registry_with_handle() -> (KindRegistry, Arc<mcphost::kinds::wasm::WasmKind>) {
    let mut kinds = KindRegistry::with_builtin();
    let wasm = Arc::new(mcphost::kinds::wasm::WasmKind::new());
    kinds.register(wasm.clone());
    (kinds, wasm)
}

/// A directory under the OS temp dir, unique per call, cleaned up on drop.
pub struct TempDataDir(pub PathBuf);

impl TempDataDir {
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("mcphost-test-{}-{}-{}", process::id(), n, nanos));
        std::fs::create_dir_all(&dir).expect("create temp data dir");
        Self(dir)
    }
}

impl Drop for TempDataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Builds `jsonschema`'s process-wide meta-schema validators on a thread with
/// a large stack, once, before any test server answers a call.
///
/// `jsonschema::validator_for` validates the schema it is given against its
/// draft's meta-schema, and the first such call in a process lazily compiles
/// ALL FIVE draft meta-schema validators (`META_SCHEMA_VALIDATORS`, a
/// `once_cell::Lazy`) -- a deeply recursive `$ref`/`$dynamicRef` walk that
/// needs on the order of 1.5 MiB of stack in an unoptimized build. Whichever
/// thread happens to validate first pays for it. In a `#[tokio::test]` that
/// thread is the test thread itself (the current-thread runtime polls the
/// server's handler tasks on it), whose stack is 2 MiB by default and already
/// holds the runtime, the rmcp/handler async frames and the call being made:
/// `cest_ac01_dry_run_verdict_matches_real_call` overflowed it (SIGABRT, "has
/// overflowed its stack") as soon as test ordering in `suite_core_07` made it
/// the first validator. Doing the one-time compile here makes every later
/// validation a shallow lookup, whichever test runs first.
pub fn prewarm_jsonschema_meta() {
    static WARMED: std::sync::Once = std::sync::Once::new();
    WARMED.call_once(|| {
        std::thread::Builder::new()
            .name("jsonschema-prewarm".to_string())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                jsonschema::validator_for(&serde_json::json!({"type": "object"}))
                    .expect("trivial schema compiles");
            })
            .expect("spawn prewarm thread")
            .join()
            .expect("jsonschema meta-schema prewarm");
    });
}

/// Runs `body` to completion on a current-thread tokio runtime that lives on
/// a dedicated thread with a 64 MiB stack, instead of on the libtest thread
/// (2 MiB unless `RUST_MIN_STACK` says otherwise). The semantics are those of
/// `#[tokio::test]` (current-thread runtime, `enable_all`); only the stack
/// differs, and a panic in `body` is re-raised on the caller so the test
/// fails normally.
///
/// For a test that drives the real `host.tool_call` path in an unoptimized
/// build. That runtime polls the server's handler tasks on the test's own
/// thread, and the handler's dispatch `async fn`s have enormous debug poll
/// frames (`McpHostHandler`'s dispatch in `src/handler.rs` is ~420 KB per
/// frame and a forwarded host verb nests it twice), so a call can need most
/// of 2 MiB before the test itself has run a line. Such a test then fails
/// with SIGABRT ("has overflowed its stack") the day an unrelated change
/// grows a frame by a few percent. Box::pin-ing the test future does not
/// help (measured): the depth is in the handler's frames, not the test's.
pub fn block_on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    let handle = std::thread::Builder::new()
        .name("big-stack-test".to_string())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build runtime")
                .block_on(body())
        })
        .expect("spawn big-stack thread");
    if let Err(panic) = handle.join() {
        std::panic::resume_unwind(panic);
    }
}

/// A bare `AppState` -- no HTTP listener, no `runs::spawn_executor`/
/// `triggers::spawn_scheduler` -- for tests that drive `mcphost::runs`/
/// `mcphost::triggers` functions directly and need the background
/// executor/scheduler *not* running (it would race their own manual
/// stranding of a `queued` run; see `runs_ac11_admin_runs_reap.rs` and
/// `sched_ac5_overlap_skips_and_records.rs`, both of which call this
/// instead of duplicating the field list themselves so a new `AppState`
/// field is added here once, not in every bare-state test).
pub async fn bare_app_state() -> (AppState, TempDataDir) {
    prewarm_jsonschema_meta();
    let data_dir = TempDataDir::new();
    let db = Db::open(&data_dir.0).expect("open db");
    db.migrate().await.expect("migrate");
    let session_bindings = mcphost::session_bind::SessionBindings::new();
    let state = AppState {
        db,
        kinds: KindRegistry::with_builtin(),
        contract: std::sync::Arc::new(mcphost::api_contract::ContractSnapshot::builtin()),
        connect_providers: mcphost::connect::ConnectProviders::from_env(),
        secrets: SecretBox::from_passphrase("test-secret-key"),
        admin_key: Some("test-admin-key".to_string()),
        public_url: "http://127.0.0.1:0".to_string(),
        alt_public_urls: Vec::new(),
        call_timeout: mcphost::state::CALL_TIMEOUT,
        registry: None,
        http_client: reqwest::Client::new(),
        sandbox_mechanism: None,
        wasm_runtime_version: None,
        tool_run_limiter: mcphost::state::ToolRunLimiter::new(),
        signup_rate_limit_per_hour: mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
        plans: mcphost::plans::PlanCatalog::default_catalog(),
        billing_config: mcphost::billing::BillingConfig::default(),
        billing_client: Arc::new(mcphost::billing::FakeBillingClient::new(mcphost::state::now_unix())),
        checkout_sessions: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        accepted_usage_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        runs: mcphost::runs::RunsRegistry::new(),
        scheduler: mcphost::triggers::SchedulerStatus::new(),
        event_counters: mcphost::hooks::EventCounters::new(),
        event_rate_limiter: mcphost::hooks::EventRateLimiter::new(),
        deprecations: Arc::new(Vec::new()),
        disk_guard: mcphost::retention::DiskGuard::from_env(),
        compat_token: None,
        build: &mcphost::build_info::BUILD,
        signup_pause: mcphost::state::SignupPause::from_env(&data_dir.0),
        claim_token_ttl_secs: mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
        claim_rate_limit_per_hour: mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
        email_config: mcphost::email::EmailConfig::default(),
        email_client: Arc::new(mcphost::email::FakeEmailClient::new()),
        owner_verify: Default::default(),
        drafter: None,
        bans: mcphost::bans::BanCache::new(),
        ban_denials_threshold: mcphost::bans::BAN_DENIALS_THRESHOLD_DEFAULT,
        ban_claim_rate_threshold: mcphost::bans::BAN_CLAIM_RATE_THRESHOLD_DEFAULT,
        oauth: mcphost::oauth::JwksCache::new(),
        oauth_allowed_algs: mcphost::oauth::parse_allowed_algs(None),
        oauth_jwks_ttl_secs: mcphost::oauth::DEFAULT_JWKS_TTL_SECS,
        authz_key: mcphost::authz::AuthzSigningKey::load_or_generate(&data_dir.0)
            .expect("authz signing key"),
        cimd_cache: mcphost::authz::CimdCache::new(),
        alerts: mcphost::alerts::AlertRegistry::new(),
        contention_tracker: mcphost::alerts::ContentionTracker::new(),
        alert_config: mcphost::alerts::AlertConfig::default(),
        alert_quota_trips: mcphost::alerts::QuotaTripTracker::new(),
        estimates: mcphost::preflight::EstimateCounter::new(),
        status_probe_override: mcphost::statusfeed::ProbeOverrides::new(),
        fleet_ips: mcphost::state::FleetIps::empty(),
        end_user_activity: Default::default(),
        oauth_healthz_cache: Default::default(),
        verified_client_ids: mcphost::state::VerifiedClientIds::empty(),
        invite_hints: mcphost::invites::InviteHintTracker::new(),
        feedback_calls: Default::default(),
        lineage_cache: mcphost::lineage::new_cache(),
        lineage_trace_pages: mcphost::lineage::new_trace_page_cache(),
        public_url_sync_deadline: mcphost::state::PUBLIC_URL_SYNC_DEADLINE,
        url_rate_limiter: mcphost::hooks::EventRateLimiter::new(),
        implicit_signup_memory: mcphost::session_bind::SessionBindings::new_sharing_secret(&session_bindings),
        tenant_key_arg_memory: mcphost::session_bind::SessionBindings::new_sharing_secret(&session_bindings),
        redeemed_peers: mcphost::peer_recent::PeerRecents::new(),
        reuse_session_tenant: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        signals: mcphost::signals::Signals::compiled(),
        anon_healthz_extra_key: Default::default(),
        blob_clock_offset_secs: Default::default(),
        confirm_clock_offset_secs: Default::default(),
        onboarding_cache: Default::default(),
        resource_subs: Default::default(),
        listen_hub: Default::default(),
        session_bindings,
    };
    (state, data_dir)
}

pub struct TestServer {
    pub base_url: String,
    pub data_dir: TempDataDir,
    pub state: Arc<AppState>,
}

impl TestServer {
    /// PRD-mcphost-data-quota-in-every-response: a server over
    /// [`bare_app_state`] whose plan catalog `mutate` edits first -- the
    /// only way to serve a plan with a chosen ceiling (`table_rows_max =
    /// 10000`) over real HTTP.
    pub async fn start_with_plans(mutate: impl FnOnce(&mut mcphost::plans::PlanCatalog)) -> Self {
        let (mut state, data_dir) = bare_app_state().await;
        mutate(&mut state.plans);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        state.public_url = format!("http://{addr}");
        let state = Arc::new(state);
        let serve_state = state.clone();
        tokio::spawn(async move {
            let _ = mcphost::http::serve_on_listener(listener, serve_state).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Self { base_url: format!("http://{addr}"), data_dir, state }
    }

    /// PRD-mcphost-owner-verification-paths: a server whose AppState carries
    /// `owner_verify` (a stubbed GitHub app and/or a fake DNS resolver).
    pub async fn start_with_owner_verify(owner_verify: mcphost::verify::OwnerVerify) -> Self {
        Self::start_with_owner_verify_and_email(owner_verify, Arc::new(mcphost::email::FakeEmailClient::new())).await
    }

    /// [`Self::start_with_owner_verify`] over a configured, inspectable fake
    /// email client, so the email path of the same tenant can be driven too.
    pub async fn start_with_owner_verify_and_email(
        owner_verify: mcphost::verify::OwnerVerify,
        email_client: Arc<mcphost::email::FakeEmailClient>,
    ) -> Self {
        let (mut state, data_dir) = bare_app_state().await;
        state.owner_verify = owner_verify;
        state.email_config = mcphost::email::EmailConfig {
            api_url: Some("https://email.invalid/send".to_string()),
            api_key: Some("test-email-key".to_string()),
            from: Some("noreply@mcphost.invalid".to_string()),
            provider: mcphost::email::EmailProvider::default(),
        };
        state.email_client = email_client;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        state.public_url = format!("http://{addr}");
        let state = Arc::new(state);
        let serve_state = state.clone();
        tokio::spawn(async move {
            let _ = mcphost::http::serve_on_listener(listener, serve_state).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Self { base_url: format!("http://{addr}"), data_dir, state }
    }

    /// PRD-mcphost-first-tool-from-description: a server whose AppState
    /// carries `drafter` (a `FakeDrafter` in every test) over `kinds`.
    pub async fn start_with_drafter(kinds: KindRegistry, drafter: Arc<dyn mcphost::drafter::Drafter>) -> Self {
        let (mut state, data_dir) = bare_app_state().await;
        state.kinds = kinds;
        state.drafter = Some(drafter);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        state.public_url = format!("http://{addr}");
        let state = Arc::new(state);
        let serve_state = state.clone();
        tokio::spawn(async move {
            let _ = mcphost::http::serve_on_listener(listener, serve_state).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Self { base_url: format!("http://{addr}"), data_dir, state }
    }

    /// PRD-mcphost-connect-start-and-callback: a server whose AppState carries
    /// `connect_providers` (host-owned OAuth credentials for `testprov`).
    pub async fn start_with_connect_providers(connect_providers: mcphost::connect::ConnectProviders) -> Self {
        let (mut state, data_dir) = bare_app_state().await;
        state.connect_providers = connect_providers;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        state.public_url = format!("http://{addr}");
        let state = Arc::new(state);
        let serve_state = state.clone();
        tokio::spawn(async move {
            let _ = mcphost::http::serve_on_listener(listener, serve_state).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Self { base_url: format!("http://{addr}"), data_dir, state }
    }

    /// Start a server with the standard test admin key.
    pub async fn start() -> Self {
        Self::start_with(Some(ADMIN_KEY.to_string())).await
    }

    pub async fn start_with(admin_key: Option<String>) -> Self {
        Self::start_full(
            admin_key,
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
        )
        .await
    }

    /// PRD-mcphost-signup-rate-configurable AC2/requirement 6: a server
    /// whose signup rate limit is overridden directly on `AppState`, the
    /// same field `$MCPHOST_SIGNUP_RATE_LIMIT_PER_HOUR` feeds at real
    /// startup -- set via the field rather than the env var itself, since
    /// mutating process environment from one of several test functions
    /// racing in the same test binary would be flaky.
    pub async fn start_with_signup_rate_limit(limit: i64) -> Self {
        Self::start_full_with_signup_rate_limit(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            limit,
        )
        .await
    }

    /// loop/mcphost-fleet-signup-limit: a server whose signup rate limit
    /// AND `$MCPHOST_FLEET_IPS`-equivalent fleet list are both overridden
    /// directly on `AppState` -- same "field, not process environment,
    /// since tests run in parallel in one binary" rationale
    /// `start_with_signup_rate_limit`'s own doc comment already states.
    /// `fleet_ips_raw` uses the same comma-separated address/CIDR syntax
    /// `$MCPHOST_FLEET_IPS` parses at real startup
    /// ([`mcphost::state::parse_fleet_ips`]).
    pub async fn start_with_signup_rate_limit_and_fleet_ips(
        limit: i64,
        fleet_ips_raw: &str,
    ) -> Self {
        Self::start_full_with_email(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            limit,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            Vec::new(),
            mcphost::email::EmailConfig::default(),
            Arc::new(mcphost::email::FakeEmailClient::new()),
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            mcphost::db::DbConfig::from_env(),
            mcphost::alerts::AlertConfig::default(),
            mcphost::state::parse_fleet_ips(Some(fleet_ips_raw)),
            mcphost::state::VerifiedClientIds::empty(),
            Vec::new(),
        )
        .await
    }

    /// PRD-mcphost-oauth-unverified-client-consent-warning AC6: a server
    /// whose operator verified-client allowlist is overridden directly on
    /// `AppState` -- same "field, not process environment, since tests run
    /// in parallel in one binary" rationale `start_with_signup_rate_limit`'s
    /// own doc comment already states. `client_ids` uses the same
    /// comma-separated syntax `$MCPHOST_VERIFIED_CLIENT_IDS` parses at real
    /// startup ([`mcphost::state::parse_verified_client_ids`]).
    pub async fn start_with_verified_client_ids(client_ids: &[&str]) -> Self {
        Self::start_full_with_email(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            Vec::new(),
            mcphost::email::EmailConfig::default(),
            Arc::new(mcphost::email::FakeEmailClient::new()),
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            mcphost::db::DbConfig::from_env(),
            mcphost::alerts::AlertConfig::default(),
            mcphost::state::FleetIps::empty(),
            mcphost::state::parse_verified_client_ids(Some(&client_ids.join(","))),
            Vec::new(),
        )
        .await
    }

    /// AC19: a server with the registry feature flag on, pointed at a
    /// mocked registry API base URL (typically a `wiremock::MockServer`'s
    /// `.uri()`).
    /// A server with a caller-supplied `KindRegistry` (the `http`-kind test
    /// suite's usual entry point: `http_kind_registry()` or
    /// `http_kind_registry_strict(...)`).
    pub async fn start_with_kinds(kinds: KindRegistry) -> Self {
        Self::start_full(
            Some(ADMIN_KEY.to_string()),
            kinds,
            mcphost::state::CALL_TIMEOUT,
            None,
        )
        .await
    }

    pub async fn start_with_registry(registry_base_url: String) -> Self {
        Self::start_full(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            Some(RegistryConfig {
                base_url: registry_base_url,
            }),
        )
        .await
    }

    /// Full control for tests that need a non-default kind registry (AC15's
    /// never-completing kind), a short call timeout (so AC15 doesn't wait
    /// out the real 30s deadline), or the registry feature flag on (AC19).
    pub async fn start_full(
        admin_key: Option<String>,
        kinds: KindRegistry,
        call_timeout: std::time::Duration,
        registry: Option<RegistryConfig>,
    ) -> Self {
        Self::start_full_with_signup_rate_limit(
            admin_key,
            kinds,
            call_timeout,
            registry,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
        )
        .await
    }

    /// PRD-grand-loop-billing: a server with billing configured (a real
    /// `MCPHOST_STRIPE_SECRET_KEY`-equivalent and/or webhook secret) and a
    /// caller-supplied [`mcphost::billing::BillingClient`] -- pass an
    /// `Arc<FakeBillingClient>` cloned before this call so the test can
    /// still inspect `last_request`/`call_count` afterward (the clone
    /// shares the same underlying `Mutex`/`AtomicUsize`, per
    /// `FakeBillingClient`'s own interior-mutability design). Every other
    /// `start_*` helper above defaults to `BillingConfig::default()` (no
    /// keys, `billing_mode: off`) and a fresh, uninspected fake client.
    pub async fn start_with_billing(
        billing_config: mcphost::billing::BillingConfig,
        billing_client: Arc<dyn mcphost::billing::BillingClient>,
    ) -> Self {
        Self::start_full_with_billing(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            billing_config,
            billing_client,
        )
        .await
    }

    /// PRD-mcphost-host-tool-deprecation: a server whose
    /// `AppState::deprecations` is set directly (never via an env var --
    /// same "field, not process environment, since tests run in parallel
    /// in one binary" rationale `start_with_signup_rate_limit`'s own doc
    /// comment already states) -- AC3/AC4/AC5's own tests each need a
    /// `contracts/deprecations.json`-equivalent entry that does NOT exist
    /// in the real, committed (empty) file.
    pub async fn start_with_deprecations(
        deprecations: Vec<mcphost::api_contract::Deprecation>,
    ) -> Self {
        Self::start_full_with_deprecations(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            deprecations,
        )
        .await
    }

    /// Same as [`Self::start_full`], with the signup rate limit also
    /// overridable (PRD-mcphost-signup-rate-configurable).
    pub async fn start_full_with_signup_rate_limit(
        admin_key: Option<String>,
        kinds: KindRegistry,
        call_timeout: std::time::Duration,
        registry: Option<RegistryConfig>,
        signup_rate_limit_per_hour: i64,
    ) -> Self {
        Self::start_full_with_billing(
            admin_key,
            kinds,
            call_timeout,
            registry,
            signup_rate_limit_per_hour,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
        )
        .await
    }

    /// Same as [`Self::start_full_with_deprecations`], with an empty
    /// deprecation ledger (every `start_*` helper above except
    /// [`Self::start_with_deprecations`] funnels in through here) --
    /// billing config and client are the two fields PRD-grand-loop-billing
    /// added to `AppState`; every other field is unchanged from before
    /// that PRD.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_full_with_billing(
        admin_key: Option<String>,
        kinds: KindRegistry,
        call_timeout: std::time::Duration,
        registry: Option<RegistryConfig>,
        signup_rate_limit_per_hour: i64,
        billing_config: mcphost::billing::BillingConfig,
        billing_client: Arc<dyn mcphost::billing::BillingClient>,
    ) -> Self {
        Self::start_full_with_deprecations(
            admin_key,
            kinds,
            call_timeout,
            registry,
            signup_rate_limit_per_hour,
            billing_config,
            billing_client,
            Vec::new(),
        )
        .await
    }

    /// PRD-mcphost-human-claim-magic-link: a server whose claim email is
    /// actually configured (a caller-supplied `Arc<FakeEmailClient>`
    /// cloned before this call so the test can inspect `sends()`/
    /// `send_count()` afterward, same "clone before, inspect after"
    /// rationale as [`Self::start_with_billing`]). Every other `start_*`
    /// helper defaults to `EmailConfig::default()` (unconfigured, AC6) and
    /// a fresh, uninspected fake client.
    pub async fn start_with_email(
        email_client: Arc<mcphost::email::FakeEmailClient>,
    ) -> Self {
        Self::start_full_with_email(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            Vec::new(),
            mcphost::email::EmailConfig {
                api_url: Some("https://email.invalid/send".to_string()),
                api_key: Some("test-email-key".to_string()),
                from: Some("noreply@mcphost.invalid".to_string()),
                provider: mcphost::email::EmailProvider::default(),
            },
            email_client,
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            mcphost::db::DbConfig::from_env(),
            mcphost::alerts::AlertConfig::default(),
            mcphost::state::FleetIps::empty(),
            mcphost::state::VerifiedClientIds::empty(),
            Vec::new(),
        )
        .await
    }

    /// PRD-mcphost-send-email-from-tool: a server with the python kind (data
    /// under `envs_dir`) and a caller-supplied, inspectable fake email client
    /// -- what a tool's `mcphost.email.send` and `host.email.send` reach.
    pub async fn start_python_with_email(
        envs_dir: &std::path::Path,
        email_client: Arc<mcphost::email::FakeEmailClient>,
    ) -> Self {
        Self::start_full_with_email(
            Some(ADMIN_KEY.to_string()),
            python_kind_registry(envs_dir),
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            Vec::new(),
            mcphost::email::EmailConfig {
                api_url: Some("https://email.invalid/send".to_string()),
                api_key: Some("test-email-key".to_string()),
                from: Some("noreply@mcphost.invalid".to_string()),
                provider: mcphost::email::EmailProvider::default(),
            },
            email_client,
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            mcphost::db::DbConfig::from_env(),
            mcphost::alerts::AlertConfig::default(),
            mcphost::state::FleetIps::empty(),
            mcphost::state::VerifiedClientIds::empty(),
            Vec::new(),
        )
        .await
    }

    /// PRD-mcphost-reachability-alt-host: same shape as
    /// [`Self::start_with_email`] (a caller-supplied, inspectable fake
    /// email client) plus `alt_public_urls` set directly on `AppState` --
    /// same "field, not process environment, since tests run in parallel
    /// in one binary" rationale as every other `start_with_*` override
    /// above. Exercises AC1 (the claim email's fallback lines).
    pub async fn start_with_email_and_alts(
        email_client: Arc<mcphost::email::FakeEmailClient>,
        alt_public_urls: Vec<String>,
    ) -> Self {
        Self::start_full_with_email(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            Vec::new(),
            mcphost::email::EmailConfig {
                api_url: Some("https://email.invalid/send".to_string()),
                api_key: Some("test-email-key".to_string()),
                from: Some("noreply@mcphost.invalid".to_string()),
                provider: mcphost::email::EmailProvider::default(),
            },
            email_client,
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            mcphost::db::DbConfig::from_env(),
            mcphost::alerts::AlertConfig::default(),
            mcphost::state::FleetIps::empty(),
            mcphost::state::VerifiedClientIds::empty(),
            alt_public_urls,
        )
        .await
    }

    /// A python-capable server that advertises `https://` public URLs
    /// (PRD-mcphost-webhook-inbox-one-call AC1/AC2).
    pub async fn start_with_kinds_https_public(kinds: KindRegistry) -> Self {
        Self::start_full_with_email_scheme(
            Some(ADMIN_KEY.to_string()),
            kinds,
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            Vec::new(),
            mcphost::email::EmailConfig::default(),
            Arc::new(mcphost::email::FakeEmailClient::new()),
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            mcphost::db::DbConfig::from_env(),
            mcphost::alerts::AlertConfig::default(),
            mcphost::state::FleetIps::empty(),
            mcphost::state::VerifiedClientIds::empty(),
            Vec::new(),
            "https",
        )
        .await
    }

    /// PRD-mcphost-reachability-alt-host: a server with
    /// `alt_public_urls` set directly on `AppState` and no other override
    /// -- for AC2/AC3/AC5 tests that don't need an inspectable email
    /// client.
    pub async fn start_with_alts(alt_public_urls: Vec<String>) -> Self {
        Self::start_full_with_email(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            Vec::new(),
            mcphost::email::EmailConfig::default(),
            Arc::new(mcphost::email::FakeEmailClient::new()),
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            mcphost::db::DbConfig::from_env(),
            mcphost::alerts::AlertConfig::default(),
            mcphost::state::FleetIps::empty(),
            mcphost::state::VerifiedClientIds::empty(),
            alt_public_urls,
        )
        .await
    }

    /// PRD-mcphost-sqlite-busy-timeout-audit AC2: a server whose
    /// `$MCPHOST_DB_BUSY_TIMEOUT_MS`-equivalent is set directly on the
    /// `Db`, never via the process environment -- same "field, not
    /// process environment, since tests run in parallel in one binary"
    /// rationale as `start_with_signup_rate_limit`'s own doc comment.
    pub async fn start_with_db_cfg(db_cfg: mcphost::db::DbConfig) -> Self {
        Self::start_full_with_email(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            Vec::new(),
            mcphost::email::EmailConfig::default(),
            Arc::new(mcphost::email::FakeEmailClient::new()),
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            db_cfg,
            mcphost::alerts::AlertConfig::default(),
            mcphost::state::FleetIps::empty(),
            mcphost::state::VerifiedClientIds::empty(),
            Vec::new(),
        )
        .await
    }

    /// PRD-mcphost-alerting-webhook: a server whose `AppState::alert_config`
    /// is set directly (never via an env var -- same "field, not process
    /// environment, since tests run in parallel in one binary" rationale
    /// `start_with_signup_rate_limit`'s own doc comment already states).
    pub async fn start_with_alert_config(alert_config: mcphost::alerts::AlertConfig) -> Self {
        Self::start_full_with_email(
            Some(ADMIN_KEY.to_string()),
            KindRegistry::with_builtin(),
            mcphost::state::CALL_TIMEOUT,
            None,
            mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
            mcphost::billing::BillingConfig::default(),
            Arc::new(mcphost::billing::FakeBillingClient::new(
                mcphost::state::now_unix(),
            )),
            Vec::new(),
            mcphost::email::EmailConfig::default(),
            Arc::new(mcphost::email::FakeEmailClient::new()),
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            mcphost::db::DbConfig::from_env(),
            alert_config,
            mcphost::state::FleetIps::empty(),
            mcphost::state::VerifiedClientIds::empty(),
            Vec::new(),
        )
        .await
    }

    /// The one real constructor every `start_*` helper above funnels
    /// into -- `deprecations` is the one field
    /// PRD-mcphost-host-tool-deprecation added to `AppState`; every other
    /// field is unchanged from before this PRD.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_full_with_deprecations(
        admin_key: Option<String>,
        kinds: KindRegistry,
        call_timeout: std::time::Duration,
        registry: Option<RegistryConfig>,
        signup_rate_limit_per_hour: i64,
        billing_config: mcphost::billing::BillingConfig,
        billing_client: Arc<dyn mcphost::billing::BillingClient>,
        deprecations: Vec<mcphost::api_contract::Deprecation>,
    ) -> Self {
        Self::start_full_with_email(
            admin_key,
            kinds,
            call_timeout,
            registry,
            signup_rate_limit_per_hour,
            billing_config,
            billing_client,
            deprecations,
            mcphost::email::EmailConfig::default(),
            Arc::new(mcphost::email::FakeEmailClient::new()),
            mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
            mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
            mcphost::db::DbConfig::from_env(),
            mcphost::alerts::AlertConfig::default(),
            mcphost::state::FleetIps::empty(),
            mcphost::state::VerifiedClientIds::empty(),
            Vec::new(),
        )
        .await
    }

    /// The one real constructor every `start_*` helper above funnels
    /// into, transitively -- `email_config`/`email_client`/
    /// `claim_token_ttl_secs`/`claim_rate_limit_per_hour` are the fields
    /// PRD-mcphost-human-claim-magic-link added to `AppState`; `db_cfg` is
    /// PRD-mcphost-sqlite-busy-timeout-audit's own addition (AC2) and
    /// `alert_config` is PRD-mcphost-alerting-webhook's own addition;
    /// `alt_public_urls` is PRD-mcphost-reachability-alt-host's own
    /// addition (every existing caller above passes `Vec::new()` -- AC5's
    /// "no alternates" default); every other field is unchanged from
    /// before those PRDs.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_full_with_email(
        admin_key: Option<String>,
        kinds: KindRegistry,
        call_timeout: std::time::Duration,
        registry: Option<RegistryConfig>,
        signup_rate_limit_per_hour: i64,
        billing_config: mcphost::billing::BillingConfig,
        billing_client: Arc<dyn mcphost::billing::BillingClient>,
        deprecations: Vec<mcphost::api_contract::Deprecation>,
        email_config: mcphost::email::EmailConfig,
        email_client: Arc<dyn mcphost::email::EmailClient>,
        claim_token_ttl_secs: i64,
        claim_rate_limit_per_hour: i64,
        db_cfg: mcphost::db::DbConfig,
        alert_config: mcphost::alerts::AlertConfig,
        fleet_ips: mcphost::state::FleetIps,
        verified_client_ids: mcphost::state::VerifiedClientIds,
        alt_public_urls: Vec<String>,
    ) -> Self {
        Self::start_full_with_email_scheme(
            admin_key,
            kinds,
            call_timeout,
            registry,
            signup_rate_limit_per_hour,
            billing_config,
            billing_client,
            deprecations,
            email_config,
            email_client,
            claim_token_ttl_secs,
            claim_rate_limit_per_hour,
            db_cfg,
            alert_config,
            fleet_ips,
            verified_client_ids,
            alt_public_urls,
            "http",
        )
        .await
    }

    /// [`Self::start_full_with_email`] with the scheme `AppState::public_url`
    /// advertises (PRD-mcphost-webhook-inbox-one-call AC1: production URLs
    /// are `https://`). The listener itself is always plain HTTP, so a test
    /// that POSTs to an advertised URL swaps the scheme back first.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_full_with_email_scheme(
        admin_key: Option<String>,
        kinds: KindRegistry,
        call_timeout: std::time::Duration,
        registry: Option<RegistryConfig>,
        signup_rate_limit_per_hour: i64,
        billing_config: mcphost::billing::BillingConfig,
        billing_client: Arc<dyn mcphost::billing::BillingClient>,
        deprecations: Vec<mcphost::api_contract::Deprecation>,
        email_config: mcphost::email::EmailConfig,
        email_client: Arc<dyn mcphost::email::EmailClient>,
        claim_token_ttl_secs: i64,
        claim_rate_limit_per_hour: i64,
        db_cfg: mcphost::db::DbConfig,
        alert_config: mcphost::alerts::AlertConfig,
        fleet_ips: mcphost::state::FleetIps,
        verified_client_ids: mcphost::state::VerifiedClientIds,
        alt_public_urls: Vec<String>,
        public_scheme: &str,
    ) -> Self {
        prewarm_jsonschema_meta();
        let data_dir = TempDataDir::new();
        let db = Db::open_with_cfg(&data_dir.0, db_cfg).expect("open db");
        db.migrate().await.expect("migrate");
        // PRD-mcphost-data-retention requirement 1: same as real `serve`
        // startup -- every test server's `retention_policy` starts from
        // whatever defaults/env this test process has, overridable per
        // test via `Db::set_retention_days_for_test`.
        db.seed_retention_policy_from_env().await.expect("seed retention policy");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}");
        let public_url = format!("{public_scheme}://{addr}");

        // PRD-mcphost-wasm-kind AC9: mirrors production's "registered ==
        // available" rule for this field -- accurate for whichever registry
        // a given test builds, without every existing `start_*` helper
        // needing to know about the `wasm` kind.
        let wasm_runtime_version = kinds
            .get("wasm")
            .map(|_| mcphost::kinds::wasm::WASM_RUNTIME_VERSION);
        let session_bindings = mcphost::session_bind::SessionBindings::new();
        let state = Arc::new(AppState {
            db,
            kinds,
            contract: Arc::new(mcphost::api_contract::ContractSnapshot::builtin()),
            connect_providers: mcphost::connect::ConnectProviders::from_env(),
            secrets: SecretBox::from_passphrase("test-secret-key"),
            admin_key,
            public_url,
            alt_public_urls,
            call_timeout,
            registry,
            http_client: reqwest::Client::new(),
            sandbox_mechanism: None,
            wasm_runtime_version,
            tool_run_limiter: mcphost::state::ToolRunLimiter::new(),
            signup_rate_limit_per_hour,
            plans: mcphost::plans::PlanCatalog::default_catalog(),
            billing_config,
            billing_client,
            checkout_sessions: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            accepted_usage_cache: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            runs: mcphost::runs::RunsRegistry::new(),
            scheduler: mcphost::triggers::SchedulerStatus::new(),
            event_counters: mcphost::hooks::EventCounters::new(),
            event_rate_limiter: mcphost::hooks::EventRateLimiter::new(),
            deprecations: std::sync::Arc::new(deprecations),
            disk_guard: mcphost::retention::DiskGuard::from_env(),
            compat_token: None,
            build: &mcphost::build_info::BUILD,
            signup_pause: mcphost::state::SignupPause::from_env(&data_dir.0),
            claim_token_ttl_secs,
            claim_rate_limit_per_hour,
            email_config,
            email_client,
            owner_verify: Default::default(),
            drafter: None,
            bans: mcphost::bans::BanCache::new(),
            ban_denials_threshold: mcphost::bans::BAN_DENIALS_THRESHOLD_DEFAULT,
            ban_claim_rate_threshold: mcphost::bans::BAN_CLAIM_RATE_THRESHOLD_DEFAULT,
            oauth: mcphost::oauth::JwksCache::new(),
            oauth_allowed_algs: mcphost::oauth::parse_allowed_algs(None),
            oauth_jwks_ttl_secs: mcphost::oauth::DEFAULT_JWKS_TTL_SECS,
            authz_key: mcphost::authz::AuthzSigningKey::load_or_generate(&data_dir.0)
                .expect("authz signing key"),
            cimd_cache: mcphost::authz::CimdCache::new(),
            alerts: mcphost::alerts::AlertRegistry::new(),
            contention_tracker: mcphost::alerts::ContentionTracker::new(),
            alert_config,
            alert_quota_trips: mcphost::alerts::QuotaTripTracker::new(),
            estimates: mcphost::preflight::EstimateCounter::new(),
            status_probe_override: mcphost::statusfeed::ProbeOverrides::new(),
            fleet_ips,
            end_user_activity: Default::default(),
            oauth_healthz_cache: Default::default(),
            verified_client_ids,
            invite_hints: mcphost::invites::InviteHintTracker::new(),
            feedback_calls: Default::default(),
            lineage_cache: mcphost::lineage::new_cache(),
            lineage_trace_pages: mcphost::lineage::new_trace_page_cache(),
            public_url_sync_deadline: mcphost::state::PUBLIC_URL_SYNC_DEADLINE,
            url_rate_limiter: mcphost::hooks::EventRateLimiter::new(),
            implicit_signup_memory: mcphost::session_bind::SessionBindings::new_sharing_secret(&session_bindings),
            tenant_key_arg_memory: mcphost::session_bind::SessionBindings::new_sharing_secret(&session_bindings),
            redeemed_peers: mcphost::peer_recent::PeerRecents::new(),
            reuse_session_tenant: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            signals: mcphost::signals::Signals::compiled(),
        anon_healthz_extra_key: Default::default(),
            blob_clock_offset_secs: Default::default(),
            confirm_clock_offset_secs: Default::default(),
            onboarding_cache: Default::default(),
            resource_subs: Default::default(),
            listen_hub: Default::default(),
            session_bindings,
        });

        // PRD-mcphost-runs-and-jobs: every test server runs the real
        // executor too, same as production -- a test that publishes a
        // python tool and calls it with `async: true` sees it actually
        // execute, not just sit `queued` forever.
        mcphost::runs::spawn_executor((*state).clone());
        // PRD-mcphost-schedules: same for the scheduler tick -- a test can
        // also call `mcphost::triggers::tick_once` directly for a
        // deterministic single tick instead of waiting on this loop's own
        // 30s cadence.
        mcphost::triggers::spawn_scheduler((*state).clone());
        // PRD-mcphost-alerting-webhook: same "every test server runs the
        // real background task too" rationale as the two spawns above --
        // AC1/AC2's pause-file tests rely on this watcher actually running.
        mcphost::alerts::spawn_pause_watch((*state).clone());
        // PRD-mcphost-docs-qa-recipe: same rationale again -- a test that
        // drives the docs-qa recipe over real HTTP against a `TestServer`
        // (rather than calling `docs_index::tick_once` directly the way
        // the docsearch_ac* unit tests do against `common::bare_state`)
        // needs the indexer's own 10s cadence actually running so its
        // `host.docs.status` poll for `lag_seconds == 0` ever completes.
        mcphost::docs_index::spawn_scheduler((*state).clone());

        let serve_state = state.clone();
        tokio::spawn(async move {
            let _ = mcphost::http::serve_on_listener(listener, serve_state).await;
        });

        // Give the listener a moment to accept.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        Self {
            base_url,
            data_dir,
            state,
        }
    }
}

/// A minimal streamable-HTTP JSON-RPC client. Every call is a fresh POST
/// (the server is stateless per 2026-07-28), so this holds no session
/// state beyond the base URL and an optional bearer key.
pub struct McpClient {
    http: reqwest::Client,
    base_url: String,
    /// PRD-mcphost-tenant-resource-metadata: the path this client posts
    /// to -- `/mcp` by default, overridable via [`Self::with_path`] to
    /// drive a `/t/{ns}/mcp` request instead.
    mcp_path: String,
    pub bearer: Option<String>,
    next_id: AtomicU64,
    /// PRD-mcphost-tenant-attribution: this client's own `clientInfo`,
    /// sent on `initialize` and (since this host runs every `tools/call`
    /// as its own stateless request, per `with_legacy_session_mode(false)`
    /// -- see `call_with_extra_header`'s existing `_meta` injection) on
    /// every subsequent call too, not just `initialize`. Defaults to what
    /// `initialize()` has always hardcoded, so every pre-existing test
    /// that never asked for a specific client keeps seeing that value.
    client_name: String,
    client_version: String,
    /// PRD-mcphost-session-bound-tenant-after-signup: opt-in
    /// [`Self::with_session_continuity`] state -- the `Mcp-Session-Id` the
    /// server issued on this client's first response, echoed on every later
    /// request, exactly as a spec-compliant streamable-HTTP client does.
    /// `None` (and never sent) unless that builder was called, so every
    /// pre-existing test keeps making one independent stateless request per
    /// call, which is the behaviour those tests were written against.
    session_id: Arc<std::sync::Mutex<Option<String>>>,
    session_continuity: bool,
}

#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub error_code: Option<String>,
    /// The full JSON-RPC error `data` object (`error_code` plus whatever
    /// extra fields the error carries, e.g. `retry_after_s`, `status`,
    /// `schema_path`) -- `error_code` above is just `data.error_code`
    /// pulled out for convenience.
    pub data: Value,
}

impl McpClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.to_string(),
            mcp_path: "/mcp".to_string(),
            bearer: None,
            next_id: AtomicU64::new(1),
            client_name: "mcphost-test".to_string(),
            client_version: "0.1.0".to_string(),
            session_id: Arc::new(std::sync::Mutex::new(None)),
            session_continuity: false,
        }
    }

    /// PRD-mcphost-session-bound-tenant-after-signup: make this client
    /// behave like a real streamable-HTTP client -- remember the
    /// `Mcp-Session-Id` the server issues and echo it on every subsequent
    /// request, so a `signup` and the calls after it share one session.
    /// Without this, each call is an independent stateless request that can
    /// never see a binding (which is what the Claude Agent SDK does today,
    /// and why AC3's anonymous behaviour is unchanged).
    pub fn with_session_continuity(mut self) -> Self {
        self.session_continuity = true;
        self
    }

    /// The session id the server issued to this client, once one has been
    /// seen. `None` before the first request, or when continuity is off.
    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().expect("session id lock").clone()
    }

    /// Adopt another client's session id -- for a test that needs a second,
    /// differently-configured client (e.g. one carrying a bearer) on the
    /// SAME session, which is what AC5's "header wins, the binding is
    /// unchanged" needs to exercise.
    pub fn joining_session_of(mut self, other: &McpClient) -> Self {
        self.session_continuity = true;
        self.session_id = Arc::clone(&other.session_id);
        self
    }

    pub fn with_bearer(base_url: &str, key: &str) -> Self {
        let mut c = Self::new(base_url);
        c.bearer = Some(key.to_string());
        c
    }

    /// PRD-mcphost-tenant-resource-metadata: point this client at
    /// `/t/{ns}/mcp` (or any other path) instead of the default `/mcp`.
    pub fn with_path(mut self, path: &str) -> Self {
        self.mcp_path = path.to_string();
        self
    }

    /// PRD-mcphost-tenant-attribution attrib_ac2/attrib_ac6: override the
    /// `clientInfo` this client sends, e.g. `{"name": "claude-code",
    /// "version": "2.1"}` -- everything else about the client is
    /// unaffected (chain onto `new`/`with_bearer`).
    pub fn with_client_info(mut self, name: &str, version: &str) -> Self {
        self.client_name = name.to_string();
        self.client_version = version.to_string();
        self
    }

    /// POST with SEP-2243 `Mcp-Method`/`Mcp-Name` headers set to match the
    /// body (`rmcp` requires this once `MCP-Protocol-Version` on the
    /// request declares `>= 2026-07-28`, the version this client always
    /// negotiates). `mcp_protocol_version_header` lets a test omit or
    /// falsify that header to exercise paths gated on its absence (see the
    /// AC13 test, which relies on it being absent so `rmcp` does not
    /// itself reject a mismatched `Mcp-Name`).
    async fn post_with(
        &self,
        body: &Value,
        mcp_protocol_version_header: Option<&str>,
        mcp_name_override: Option<&str>,
    ) -> reqwest::Response {
        self.post_with_extra(body, mcp_protocol_version_header, mcp_name_override, &[])
            .await
    }

    /// Same as [`Self::post_with`], plus zero or more arbitrary extra
    /// headers -- PRD-mcphost-synthetic-flag's `x-mcphost-synthetic` was
    /// the first user (see [`Self::tools_call_with_header`]); PRD-mcphost-funnel-truth's
    /// `x-mcphost-origin` needs a SECOND header (`x-forwarded-for`) on the
    /// same call (see [`Self::tools_call_with_headers`]), which is why this
    /// takes a slice rather than the single `Option<(&str, &str)>` it used
    /// to.
    async fn post_with_extra(
        &self,
        body: &Value,
        mcp_protocol_version_header: Option<&str>,
        mcp_name_override: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> reqwest::Response {
        let method = body.get("method").and_then(Value::as_str).unwrap_or("");
        let mcp_name = mcp_name_override.map(str::to_string).or_else(|| {
            body.get("params")
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .filter(|_| method == "tools/call")
                .map(str::to_string)
        });

        let mut req = self
            .http
            .post(format!("{}{}", self.base_url, self.mcp_path))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(body);
        if let Some(version) = mcp_protocol_version_header {
            req = req.header("MCP-Protocol-Version", version);
        }
        if method != "initialize" && !method.is_empty() {
            req = req.header("Mcp-Method", method);
        }
        if let Some(name) = mcp_name {
            req = req.header("Mcp-Name", name);
        }
        if let Some(key) = &self.bearer {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        for (name, value) in extra_headers {
            req = req.header(*name, *value);
        }
        if self.session_continuity
            && let Some(id) = self.session_id()
        {
            req = req.header("Mcp-Session-Id", id);
        }
        let response = req.send().await.expect("send request");
        if self.session_continuity
            && let Some(id) = response
                .headers()
                .get("Mcp-Session-Id")
                .and_then(|v| v.to_str().ok())
        {
            *self.session_id.lock().expect("session id lock") = Some(id.to_string());
        }
        response
    }

    async fn post(&self, body: Value) -> reqwest::Response {
        self.post_with(&body, Some("2026-07-28"), None).await
    }

    /// Raw POST for tests that need to inspect status/headers directly.
    pub async fn post_raw(&self, body: Value) -> reqwest::Response {
        self.post(body).await
    }

    /// POST that omits `MCP-Protocol-Version` (so `rmcp`'s SEP-2243
    /// enforcement is skipped) and sends the given `Mcp-Name` header
    /// regardless of the body — for the AC13 mismatch test.
    pub async fn post_with_mcp_name_override(
        &self,
        body: Value,
        mcp_name: &str,
    ) -> reqwest::Response {
        self.post_with(&body, None, Some(mcp_name)).await
    }

    pub async fn initialize(&self) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let resp = self
            .post(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2026-07-28",
                    "capabilities": {},
                    "clientInfo": {"name": self.client_name, "version": self.client_version}
                }
            }))
            .await;
        let body: Value = resp.json().await.expect("parse initialize response");
        body
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.call_with_extra_headers(method, params, &[]).await
    }

    async fn call_with_extra_headers(
        &self,
        method: &str,
        mut params: Value,
        extra_headers: &[(&str, &str)],
    ) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // Stateless 2026-07-28 requests carry the client context SEP-2575
        // requires on every request (no session to remember it from
        // `initialize`), not just in `initialize` itself. PRD-mcphost-tenant-attribution:
        // `io.modelcontextprotocol/clientInfo` rides along here too, now
        // that `host::peer_client_info` (via `RequestContext::client_info`)
        // reads it -- every pre-existing test that never called
        // `with_client_info` keeps sending the same `mcphost-test`/`0.1.0`
        // pair `initialize` has always hardcoded.
        if let Some(obj) = params.as_object_mut() {
            obj.insert(
                "_meta".to_string(),
                json!({
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                    "io.modelcontextprotocol/clientInfo": {
                        "name": self.client_name,
                        "version": self.client_version,
                    },
                }),
            );
        }
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let resp = self
            .post_with_extra(&body, Some("2026-07-28"), None, extra_headers)
            .await;
        let status = resp.status();
        // PRD-mcphost-one-next-tool requirement 3 (AC3): a call that binds
        // this session (`signup`/`host.redeem`) now emits
        // `notifications/tools/list_changed` before its own result --
        // `rmcp`'s own `StreamableHttpServerConfig::json_response` doc says
        // a handler that emits anything before its final response forces
        // that one response to upgrade from plain `application/json` to
        // `text/event-stream` carrying both, in order, so every existing
        // caller of this shared helper (effectively every test that calls
        // `signup`) needs to keep working against either shape. The real
        // JSON-RPC result is always the LAST message on the wire -- any
        // notification comes first (see `tools_call_collect_messages`,
        // which keeps the whole ordered sequence for the dedicated AC3
        // test).
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body: Value = if content_type.starts_with("text/event-stream") {
            let text = resp
                .text()
                .await
                .unwrap_or_else(|e| panic!("read {method} SSE body ({status}): {e}"));
            parse_sse_messages(&text)
                .pop()
                .unwrap_or_else(|| panic!("{method} SSE body carried no JSON-RPC message: {text}"))
        } else {
            resp.json()
                .await
                .unwrap_or_else(|e| panic!("parse {method} response ({status}): {e}"))
        };
        if let Some(error) = body.get("error") {
            return Err(RpcError {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                error_code: error
                    .get("data")
                    .and_then(|d| d.get("error_code"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                data: error.get("data").cloned().unwrap_or(Value::Null),
            });
        }
        let result = body.get("result").cloned().unwrap_or(Value::Null);
        if method == "tools/call"
            && let Some(name) = params.get("name").and_then(Value::as_str)
        {
            outsch_capture(name, &result);
        }
        Ok(result)
    }

    /// PRD-mcphost-server-capabilities-from-registry: send any JSON-RPC
    /// `method` (stateless 2026-07-28 shape, same `_meta` as [`Self::call`])
    /// and return the whole response body -- `result` or `error` -- so a
    /// test can assert on a `-32601` as readily as on a result.
    pub async fn rpc(&self, method: &str, mut params: Value) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Some(obj) = params.as_object_mut() {
            obj.insert(
                "_meta".to_string(),
                json!({
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                    "io.modelcontextprotocol/clientInfo": {
                        "name": self.client_name,
                        "version": self.client_version,
                    },
                }),
            );
        }
        // SEP-2243: methods that name a resource/prompt must carry it as
        // `Mcp-Name` (`post_with_extra` only derives it for `tools/call`).
        let mcp_name = params
            .get("uri")
            .or_else(|| params.get("name"))
            .and_then(Value::as_str)
            .filter(|_| method != "tools/call")
            .map(str::to_string);
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let resp = self
            .post_with_extra(&body, Some("2026-07-28"), mcp_name.as_deref(), &[])
            .await;
        let status = resp.status();
        let is_sse = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.starts_with("text/event-stream"));
        if is_sse {
            let text = resp.text().await.unwrap_or_else(|e| panic!("read {method} SSE ({status}): {e}"));
            parse_sse_messages(&text)
                .pop()
                .unwrap_or_else(|| panic!("{method} SSE body carried no message: {text}"))
        } else {
            resp.json().await.unwrap_or_else(|e| panic!("parse {method} response ({status}): {e}"))
        }
    }

    pub async fn tools_list(&self) -> Result<Value, RpcError> {
        self.call("tools/list", json!({})).await
    }

    pub async fn tools_call(&self, name: &str, arguments: Value) -> Result<Value, RpcError> {
        self.call("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }

    /// PRD-mcphost-synthetic-flag: a `tools/call` carrying one extra raw
    /// header, e.g. `("x-mcphost-synthetic", "synthorg:run-a")` on a
    /// `signup` call -- the only header this suite needs to set outside
    /// `Mcp-Name`/`Authorization`, which `post_with_extra` already handles.
    pub async fn tools_call_with_header(
        &self,
        name: &str,
        arguments: Value,
        header: (&str, &str),
    ) -> Result<Value, RpcError> {
        self.tools_call_with_headers(name, arguments, &[header]).await
    }

    /// PRD-mcphost-funnel-truth AC3: same as [`Self::tools_call_with_header`],
    /// for a test that needs BOTH `x-forwarded-for` (to simulate a fleet or
    /// non-fleet source IP) and `x-mcphost-origin` on the same `signup`
    /// call.
    pub async fn tools_call_with_headers(
        &self,
        name: &str,
        arguments: Value,
        headers: &[(&str, &str)],
    ) -> Result<Value, RpcError> {
        self.call_with_extra_headers(
            "tools/call",
            json!({"name": name, "arguments": arguments}),
            headers,
        )
        .await
    }

    /// PRD-mcphost-one-next-tool AC3: calls a tool and returns every
    /// JSON-RPC message the wire response carried, in order -- a plain
    /// `application/json` response (the common case) comes back as the
    /// one-element list `[result-or-error]`; a `text/event-stream` response
    /// (forced when the handler emits `notifications/tools/list_changed`
    /// before its own result, see `call_with_extra_header`'s own doc
    /// comment) comes back with the notification(s) first and the real
    /// response last, exactly the ordering AC3 needs to prove.
    pub async fn tools_call_collect_messages(&self, name: &str, arguments: Value) -> Vec<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let params = json!({
            "name": name,
            "arguments": arguments,
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {
                    "name": self.client_name,
                    "version": self.client_version,
                },
            },
        });
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": params,
        });
        let resp = self
            .post_with_extra(&body, Some("2026-07-28"), Some(name), &[])
            .await;
        let status = resp.status();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let text = resp
            .text()
            .await
            .unwrap_or_else(|e| panic!("read tools/call body ({status}): {e}"));
        if content_type.starts_with("text/event-stream") {
            parse_sse_messages(&text)
        } else {
            vec![serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse tools/call response ({status}): {e}"))]
        }
    }
}

/// PRD-mcphost-composition `compose_ac*.rs` suite: `host.tool_publish` a
/// tool of `kind` under local `name`, returning the qualified
/// `<namespace>.<name>` `tools/call` would use. Panics on a publish
/// failure (every caller in that suite expects success; a test asserting a
/// publish-time rejection calls `host.tool_publish` directly instead).
/// [`publish`] with `force: true` -- for a test that deliberately publishes a
/// chain whose mapping the publish gate would refuse as `will_fail`
/// (PRD-mcphost-chain-publish-time-path-check requirement 3).
pub async fn publish_forced(client: &McpClient, name: &str, kind: &str, spec: Value) -> String {
    let result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": name, "kind": kind, "spec": spec, "force": true}),
        )
        .await
        .unwrap_or_else(|e| panic!("forced publish '{name}' failed: {} {}", e.code, e.message));
    extract_structured(&result)["name"]
        .as_str()
        .expect("published tool's qualified name")
        .to_string()
}

pub async fn publish(client: &McpClient, name: &str, kind: &str, spec: Value) -> String {
    let result = client
        .tools_call(
            "host.tool_publish",
            json!({"name": name, "kind": kind, "spec": spec}),
        )
        .await
        .unwrap_or_else(|e| panic!("publish '{name}' failed: {} {}", e.code, e.message));
    extract_structured(&result)["name"]
        .as_str()
        .expect("published tool's qualified name")
        .to_string()
}

/// Sign up a fresh tenant against a running server and return
/// `(tenant_namespace, key)`.
pub async fn signup(base_url: &str, display_name: &str) -> (String, String) {
    let client = McpClient::new(base_url);
    let result = client
        .tools_call("signup", json!({"name": display_name}))
        .await
        .unwrap_or_else(|e| panic!("signup failed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    let tenant = structured["tenant"]
        .as_str()
        .expect("tenant field")
        .to_string();
    let key = structured["key"].as_str().expect("key field").to_string();
    (tenant, key)
}

/// Same as [`signup`], but the `signup` call carries an
/// `x-mcphost-synthetic: <header_value>` header -- returns the raw
/// `signup` result `Value` (not just `(tenant, key)`) so a test can also
/// assert on its shape (AC2's "byte-identical to an unlabeled signup's
/// shape" needs the whole response).
pub async fn signup_with_synthetic_header(
    base_url: &str,
    display_name: &str,
    header_value: &str,
) -> Value {
    let client = McpClient::new(base_url);
    let result = client
        .tools_call_with_header(
            "signup",
            json!({"name": display_name}),
            ("x-mcphost-synthetic", header_value),
        )
        .await
        .unwrap_or_else(|e| panic!("signup failed: {} {}", e.code, e.message));
    extract_structured(&result)
}

/// Polls `tools_call(qualified_name, args)` until it stops reporting a
/// still-building environment, or `timeout` elapses.
///
/// PRD-mcphost-first-call-reliability requirement 1 made `kinds::python`'s
/// `call` itself wait (bounded by `MCPHOST_CALL_READY_WAIT_MS`, default
/// 20s) for a building environment before giving up, so a not-yet-ready
/// environment no longer surfaces as a `tool_building`-coded `RpcError` --
/// it comes back as a successful JSON-RPC response carrying the structured
/// `{status: "building", retry_after_ms, ready_check}` result (requirement
/// 2). This helper still recognizes the old error-coded shape too (belt-
/// and-suspenders, same reasoning as `call_through_build` in
/// `kinds/python.rs`'s own tests) so it keeps working unmodified if a
/// future kind reintroduces it.
pub async fn poll_until_ready(
    client: &McpClient,
    qualified_name: &str,
    args: Value,
    timeout: std::time::Duration,
) -> Result<Value, RpcError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let result = client.tools_call(qualified_name, args.clone()).await;
        let is_building = match &result {
            Err(e) => e.error_code.as_deref() == Some("tool_building"),
            Ok(v) => extract_structured(v)["status"] == json!("building"),
        };
        if !is_building || std::time::Instant::now() >= deadline {
            return result;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// PRD-mcphost-tool-test: `host.spec_test`'s own cold-start equivalent of
/// [`poll_until_ready`] -- a `python` spec's first invocation hits the same
/// "environment still building" state a published call's first invocation
/// would (see that function's docs), but `host.spec_test` never propagates
/// it as an `RpcError`: it's captured per-invocation as `{"ok": false,
/// "code": "tool_building", ...}` inside an otherwise-successful response
/// (PRD-mcphost-tool-test AC2's "the JSON-RPC call as a whole succeeds").
/// Polls the whole `host.spec_test` call until no invocation in the
/// response still reports `tool_building`, or `timeout` elapses.
pub async fn poll_spec_test_until_ready(
    client: &McpClient,
    body: Value,
    timeout: std::time::Duration,
) -> Value {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let result = client
            .tools_call("host.spec_test", body.clone())
            .await
            .expect("host.spec_test ok");
        let structured = extract_structured(&result);
        let still_building = structured["invocations"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|inv| inv["code"] == json!("tool_building"));
        if !still_building || std::time::Instant::now() >= deadline {
            return result;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// PRD-mcphost-metered-overage's `metering_ac*.rs` suite: sign up a fresh
/// tenant, then directly promote it to `pro` with a `stripe_customer_id` on
/// file (the state `mcphost billing emit-meter` requires per tenant) --
/// bypassing a real Stripe checkout/webhook round trip, since these tests
/// are about the meter-emission path, not the upgrade path (grand-loop-
/// billing's `billing_ac*.rs` suite already covers that). Returns
/// `(tenant_namespace, key, tenant_id)`.
pub async fn signup_and_make_pro(
    server: &TestServer,
    display_name: &str,
    stripe_customer_id: &str,
) -> (String, String, i64) {
    let (ns, key) = signup(&server.base_url, display_name).await;
    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns.clone())
        .await
        .expect("find tenant")
        .expect("tenant exists");
    server
        .state
        .db
        .upgrade_tenant_plan(
            tenant.id,
            "pro".to_string(),
            mcphost::state::rfc3339_now(),
            None,
        )
        .await
        .expect("upgrade to pro");
    server
        .state
        .db
        .set_stripe_customer_id(tenant.id, stripe_customer_id.to_string())
        .await
        .expect("set stripe_customer_id");
    (ns, key, tenant.id)
}

/// Record `n` successful (`ok = 1`) calls for `tenant_id` directly against
/// the database -- the metering suite doesn't need a real tool call per
/// row, just rows in `calls` for `Db::pending_meter_groups` to read.
pub async fn record_ok_calls(server: &TestServer, tenant_id: i64, tool_name: &str, n: usize) {
    for _ in 0..n {
        server
            .state
            .db
            .record_call(
                tenant_id,
                tool_name.to_string(),
                1,
                true,
                None,
                None,
                None,
                "ok",
                "external".to_string(),
                None,
            )
            .await
            .expect("record_call");
    }
}

/// PRD-mcphost-wasm-kind: base64-encodes one of the prebuilt component
/// fixtures under `tests/fixtures/wasm/<name>.wasm` (see
/// `tests/fixtures/wasm-src/<name>/` for the guest source they were built
/// from via `cargo component build --release`), ready to drop straight into
/// a `{"component": ...}` spec.
pub fn wasm_fixture_b64(name: &str) -> String {
    use base64::Engine as _;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm")
        .join(format!("{name}.wasm"));
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("read wasm fixture {}: {e}", path.display()));
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// PRD-mcphost-document-store AC3/AC6: an owned (not `Arc`-wrapped)
/// `AppState` over a scratch data dir, the same literal `tables.rs`'s own
/// `#[cfg(test)]` `bare_state()` builds -- duplicated here (rather than
/// imported) because that one lives in the lib crate's own unit tests,
/// unreachable from this integration-test crate. Needed for two AC shapes
/// `TestServer`'s real HTTP transport can't exercise: a `put` at/over
/// `MCPHOST_DOC_MAX_BYTES` (2 MiB by default), which is bigger than the
/// unrelated, already-pinned `MAX_REQUEST_BODY_BYTES` (1 MiB, PRD-mcphost-
/// call-limits-honest) transport cap every `tools/call` request shares; and
/// a plan quota overridden to a small number directly on an owned
/// `AppState.plans` (an `Arc<AppState>`, which `TestServer` hands back,
/// can't be mutated after construction).
pub async fn bare_state(dir: &std::path::Path) -> AppState {
    prewarm_jsonschema_meta();
    let db = mcphost::db::Db::open(dir).expect("open db");
    db.migrate().await.expect("migrate");
    let session_bindings = mcphost::session_bind::SessionBindings::new();
    AppState {
        db,
        kinds: mcphost::kinds::KindRegistry::with_builtin(),
        contract: Arc::new(mcphost::api_contract::ContractSnapshot::builtin()),
        connect_providers: mcphost::connect::ConnectProviders::from_env(),
        secrets: SecretBox::from_passphrase("test-secret-key"),
        admin_key: Some(ADMIN_KEY.to_string()),
        public_url: "http://127.0.0.1:0".to_string(),
        alt_public_urls: Vec::new(),
        call_timeout: mcphost::state::CALL_TIMEOUT,
        registry: None,
        http_client: reqwest::Client::new(),
        sandbox_mechanism: None,
        wasm_runtime_version: None,
        tool_run_limiter: mcphost::state::ToolRunLimiter::new(),
        signup_rate_limit_per_hour: mcphost::state::SIGNUP_RATE_LIMIT_PER_HOUR,
        plans: mcphost::plans::PlanCatalog::default_catalog(),
        billing_config: mcphost::billing::BillingConfig::default(),
        billing_client: std::sync::Arc::new(mcphost::billing::FakeBillingClient::new(
            mcphost::state::now_unix(),
        )),
        checkout_sessions: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        accepted_usage_cache: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        runs: mcphost::runs::RunsRegistry::new(),
        scheduler: mcphost::triggers::SchedulerStatus::new(),
        event_counters: mcphost::hooks::EventCounters::new(),
        event_rate_limiter: mcphost::hooks::EventRateLimiter::new(),
        deprecations: std::sync::Arc::new(Vec::new()),
        disk_guard: mcphost::retention::DiskGuard::from_env(),
        compat_token: None,
        build: &mcphost::build_info::BUILD,
        signup_pause: mcphost::state::SignupPause::from_env(dir),
        claim_token_ttl_secs: mcphost::state::CLAIM_TOKEN_TTL_SECS_DEFAULT,
        claim_rate_limit_per_hour: mcphost::state::CLAIM_RATE_LIMIT_PER_HOUR_DEFAULT,
        email_config: mcphost::email::EmailConfig::default(),
        email_client: std::sync::Arc::new(mcphost::email::FakeEmailClient::new()),
        owner_verify: Default::default(),
        drafter: None,
        bans: mcphost::bans::BanCache::new(),
        ban_denials_threshold: mcphost::bans::BAN_DENIALS_THRESHOLD_DEFAULT,
        ban_claim_rate_threshold: mcphost::bans::BAN_CLAIM_RATE_THRESHOLD_DEFAULT,
        oauth: mcphost::oauth::JwksCache::new(),
        oauth_allowed_algs: mcphost::oauth::parse_allowed_algs(None),
        oauth_jwks_ttl_secs: mcphost::oauth::DEFAULT_JWKS_TTL_SECS,
        authz_key: mcphost::authz::AuthzSigningKey::load_or_generate(dir).expect("authz signing key"),
        cimd_cache: mcphost::authz::CimdCache::new(),
        alerts: mcphost::alerts::AlertRegistry::new(),
        contention_tracker: mcphost::alerts::ContentionTracker::new(),
        alert_config: mcphost::alerts::AlertConfig::default(),
        alert_quota_trips: mcphost::alerts::QuotaTripTracker::new(),
        estimates: mcphost::preflight::EstimateCounter::new(),
        status_probe_override: mcphost::statusfeed::ProbeOverrides::new(),
        fleet_ips: mcphost::state::FleetIps::empty(),
        end_user_activity: Default::default(),
        oauth_healthz_cache: Default::default(),
        verified_client_ids: mcphost::state::VerifiedClientIds::empty(),
        invite_hints: mcphost::invites::InviteHintTracker::new(),
        feedback_calls: Default::default(),
        lineage_cache: mcphost::lineage::new_cache(),
        lineage_trace_pages: mcphost::lineage::new_trace_page_cache(),
        public_url_sync_deadline: mcphost::state::PUBLIC_URL_SYNC_DEADLINE,
        url_rate_limiter: mcphost::hooks::EventRateLimiter::new(),
        implicit_signup_memory: mcphost::session_bind::SessionBindings::new_sharing_secret(&session_bindings),
        tenant_key_arg_memory: mcphost::session_bind::SessionBindings::new_sharing_secret(&session_bindings),
        redeemed_peers: mcphost::peer_recent::PeerRecents::new(),
        reuse_session_tenant: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        signals: mcphost::signals::Signals::compiled(),
        anon_healthz_extra_key: Default::default(),
        blob_clock_offset_secs: Default::default(),
        confirm_clock_offset_secs: Default::default(),
        onboarding_cache: Default::default(),
        resource_subs: Default::default(),
        listen_hub: Default::default(),
        session_bindings,
    }
}

/// PRD-mcphost-capabilities-generated-from-dispatch R5: a raw JSON-RPC POST
/// that sends NO SEP-2243 headers (no `Mcp-Method`, `Mcp-Name`, nor
/// `MCP-Protocol-Version`) unless `mcp_method_header` asks for `Mcp-Method`
/// -- what the Claude Agent SDK and the python-mcp SDK actually put on the
/// wire, deliberately independent of [`McpClient`].
pub async fn raw_rpc(
    base_url: &str,
    path: &str,
    method: &str,
    params: Value,
    mcp_method_header: bool,
) -> Value {
    let mut req = reqwest::Client::new()
        .post(format!("{base_url}{path}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string());
    if mcp_method_header {
        req = req.header("Mcp-Method", method);
    }
    let resp = req.send().await.expect("raw_rpc send");
    let text = resp.text().await.expect("raw_rpc body");
    if text.trim_start().starts_with('{') {
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("raw_rpc: reply not JSON ({e}): {text}"))
    } else {
        parse_sse_messages(&text).pop().unwrap_or_else(|| panic!("raw_rpc: empty reply: {text:?}"))
    }
}

/// A fresh tenant in `state`'s own db, for direct `docs::doc_*` calls
/// against a [`bare_state`].
pub async fn bare_tenant(state: &AppState, namespace: &str) -> mcphost::db::Tenant {
    state
        .db
        .create_tenant(
            format!("{namespace} display"),
            namespace.to_string(),
            format!("key-hash-{namespace}"),
            None,
        )
        .await
        .expect("create_tenant")
}

/// PRD-mcphost-one-next-tool requirement 3 (AC3): parse a `text/event-stream`
/// response body into its ordered JSON-RPC messages -- each event is a
/// `data: <json>` line (`rmcp`'s own wire format, no `event:` line since
/// every message here is the default `message` type), blocks separated by a
/// blank line. Used both by [`McpClient::call_with_extra_header`] (which
/// only needs the last message, the real result) and
/// [`McpClient::tools_call_collect_messages`] (which needs the whole
/// sequence, notification(s) included, to prove ordering).
pub fn parse_sse_messages(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .filter_map(|block| {
            block.lines().find_map(|line| {
                line.strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
            })
        })
        .filter(|data| !data.is_empty())
        .map(|data| serde_json::from_str(data).unwrap_or_else(|e| panic!("parse SSE data as JSON: {e}: {data}")))
        .collect()
}

/// PRD-mcphost-implicit-signup: a bare call that implicitly signs up now
/// binds the session and emits `notifications/tools/list_changed` before
/// its own result, same as `signup`/`host.redeem` always have -- which
/// upgrades the response from plain `application/json` to
/// `text/event-stream` (see [`McpClient::call_with_extra_header`]'s own
/// comment on the same thing). A raw [`reqwest::Response`] obtained via
/// [`McpClient::post_with_mcp_name_override`]/[`McpClient::post_raw`] must
/// branch on `Content-Type` the same way that helper does instead of
/// assuming plain JSON, or parsing panics on an implicitly-signed-up call's
/// response. The real JSON-RPC message is always the last one on the wire.
pub async fn parse_response_body(resp: reqwest::Response) -> Value {
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if content_type.starts_with("text/event-stream") {
        let text = resp
            .text()
            .await
            .unwrap_or_else(|e| panic!("read SSE body ({status}): {e}"));
        parse_sse_messages(&text)
            .pop()
            .unwrap_or_else(|| panic!("SSE body carried no JSON-RPC message: {text}"))
    } else {
        resp.json()
            .await
            .unwrap_or_else(|e| panic!("parse response body ({status}): {e}"))
    }
}

/// `CallToolResult::structured` puts the value in `structuredContent`;
/// fall back to parsing the first text content block for safety.
/// PRD-mcphost-call-trace-replay: a `host.tool_call` result now carries
/// `call_id`; a test pinning the tool's own output drops it first.
pub fn without_call_id(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.remove("call_id");
    }
    value
}

pub fn extract_structured(call_result: &Value) -> Value {
    if let Some(sc) = call_result.get("structuredContent") {
        return sc.clone();
    }
    if let Some(content) = call_result.get("content").and_then(Value::as_array)
        && let Some(first) = content.first()
        && let Some(text) = first.get("text").and_then(Value::as_str)
    {
        return serde_json::from_str(text).unwrap_or(Value::Null);
    }
    Value::Null
}

/// Process-wide serialization for tests that mutate a process env var,
/// generalising the `AdvisoryModeGuard` pattern (originally specific to
/// `MCPHOST_ADVISORY_MODE`) to any key.
///
/// `scripts/gen-test-suites.sh` consolidates many `tests/ac*.rs` files into
/// a handful of suite binaries (see e.g. `tests/suite_sandbox_02.rs`), so a
/// test file that is the *only* one setting a var within its own source file
/// can still share a process -- and run concurrently, on its own tokio-test
/// task -- with another such file bundled into the same suite. Without this
/// guard, two tests racing the same var can each observe the other's value
/// mid-call (`known_advisory_fails_publish_when_mode_is_fail`, AC3, expects
/// `fail`, raced
/// `admin_usage_tallies_tools_by_network_mode_and_advisory_state`, AC9,
/// expects `warn`, flaking AC3's "must fail" assertion about 1 run in 3).
///
/// Acquire via [`EnvGuard::set`] and hold the returned guard for the test's
/// entire env-dependent span (letting it drop at end of test is simplest
/// and correct); the `Drop` impl restores the var's prior value (or removes
/// it if unset before) and releases the lock together, so no sibling test
/// can observe a stale value or start its own span early. One process-wide
/// lock, not one per key: two tests guarding two *different* keys still
/// serialize against each other, which is strictly safer than racing (and
/// guarded tests are a small minority of the suite, so the lost parallelism
/// doesn't matter).
static ENV_GUARD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
    _permit: tokio::sync::MutexGuard<'static, ()>,
}

impl EnvGuard {
    pub async fn set(key: &'static str, value: &str) -> Self {
        let permit = ENV_GUARD_LOCK.lock().await;
        let prev = std::env::var(key).ok();
        // SAFETY: `permit` serializes every test that touches any guarded
        // var -- no sibling test can be between its own set_var/remove_var
        // while this guard is held, so this process-wide mutation has no
        // concurrent reader/writer.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, prev, _permit: permit }
    }

    /// Like [`set`](Self::set), but ensures `key` is absent rather than
    /// giving it a value (e.g. simulating "no proxy configured").
    pub async fn clear(key: &'static str) -> Self {
        let permit = ENV_GUARD_LOCK.lock().await;
        let prev = std::env::var(key).ok();
        // SAFETY: see `set` above.
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, prev, _permit: permit }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: see `set` above -- still holding `_permit`.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

/// Thin alias over [`EnvGuard`], kept so existing call sites
/// (`common::AdvisoryModeGuard::set(mode)`) don't need to name the var.
pub struct AdvisoryModeGuard;

impl AdvisoryModeGuard {
    pub async fn set(mode: &str) -> EnvGuard {
        EnvGuard::set("MCPHOST_ADVISORY_MODE", mode).await
    }
}

/// `perf_budget!(budget_ms, { body })` — PRD-mcphost-test-suite-flake-lints
/// requirement 3. Runs `body` once as an untimed warm-up (so lazy init or a
/// cold cache doesn't skew the first timed sample), then
/// [`PERF_BUDGET_SAMPLES`] timed runs, and asserts the MEDIAN elapsed time
/// is at most `budget_ms` — a single measurement on a loaded shared builder
/// measures the builder's load, not the code under test (the
/// `enduserctl_ac10` false red: 52ms once against a 50ms budget, run 309).
///
/// Skips (prints `perf skipped (load)`, passes without measuring) when
/// `$MCPHOST_PERF_SKIP=1`, which the gate sets when `/proc/loadavg`'s load1
/// exceeds nproc/2; the final gate re-runs the same test on a quiet box
/// with the var unset. Always prints the measured median (or the skip) to
/// stderr, so the gate log carries the number either way.
///
/// `#[macro_export]` always hoists a macro to the DEFINING crate's root
/// (module nesting is irrelevant to it), so call sites use
/// `crate::perf_budget!(...)`, not `common::perf_budget!(...)`.
pub const PERF_BUDGET_SAMPLES: usize = 5;

/// True when `MCPHOST_PERF_SKIP=1` (the gate sets it on a loaded host). The ONE
/// place the variable is read for timing assertions: `perf_budget!`,
/// `perf_overhead_budget!`, and tests whose timing assertion cannot be
/// re-run as a repeated body (one-shot publish, concurrent p95) call this.
pub fn perf_skipped() -> bool {
    std::env::var("MCPHOST_PERF_SKIP").ok().as_deref() == Some("1")
}

/// Shared sampler: untimed warm-up, then [`PERF_BUDGET_SAMPLES`] timed runs of
/// `body`; evaluates to the MEDIAN elapsed `Duration`. A macro (not a fn) so
/// `body` may `.await`. Used by `perf_budget!` and `perf_overhead_budget!`.
#[macro_export]
macro_rules! perf_median {
    ($body:block) => {{
        // Untimed warm-up.
        { $body }
        let mut samples: Vec<std::time::Duration> =
            Vec::with_capacity($crate::common::PERF_BUDGET_SAMPLES);
        for _ in 0..$crate::common::PERF_BUDGET_SAMPLES {
            let __perf_start = std::time::Instant::now();
            { $body }
            samples.push(__perf_start.elapsed());
        }
        samples.sort();
        samples[samples.len() / 2]
    }};
}

/// timing-helper
#[macro_export]
macro_rules! perf_budget {
    ($budget_ms:expr, $body:block) => {{
        if $crate::common::perf_skipped() {
            eprintln!("perf skipped (load)");
        } else {
            let median = $crate::perf_median!($body);
            let budget = std::time::Duration::from_millis($budget_ms as u64);
            eprintln!("perf_budget: median={median:?} budget={budget:?}");
            assert!(
                median <= budget,
                "perf budget exceeded: median {median:?} over {} samples > budget {budget:?}",
                $crate::common::PERF_BUDGET_SAMPLES,
            );
        }
    }};
}

/// `perf_overhead_budget!(budget_ms, { base }, { measured })` — asserts that
/// `median(measured) - median(base)` (saturating at zero) is at most
/// `budget_ms`, for budgets on the OVERHEAD a feature adds (e.g. query
/// logging) rather than on a whole call. Same skip (`MCPHOST_PERF_SKIP=1`),
/// warm-up and median sampling as `perf_budget!` (shared `perf_median!`).
/// timing-helper
#[macro_export]
macro_rules! perf_overhead_budget {
    ($budget_ms:expr, $base:block, $measured:block) => {{
        if $crate::common::perf_skipped() {
            eprintln!("perf skipped (load)");
        } else {
            let base = $crate::perf_median!($base);
            let measured = $crate::perf_median!($measured);
            let overhead = measured.saturating_sub(base);
            let budget = std::time::Duration::from_millis($budget_ms as u64);
            eprintln!(
                "perf_overhead_budget: base={base:?} measured={measured:?} overhead={overhead:?} budget={budget:?}"
            );
            assert!(
                overhead <= budget,
                "perf overhead budget exceeded: overhead {overhead:?} (measured {measured:?} - base {base:?}, median of {} samples) > budget {budget:?}",
                $crate::common::PERF_BUDGET_SAMPLES,
            );
        }
    }};
}

/// `deadline_budget!(budget_ms, { body })` — PRD-mcphost-timing-assertion-lint
/// requirement 3: a hang-guard DEADLINE, not a performance budget. Runs `body`
/// ONCE (no warm-up, no median -- the bodies are one-shot calls that cannot be
/// repeated) and asserts it finished within `budget_ms`, scaled x3 when
/// `$MCPHOST_PERF_SKIP=1` marks the host as loaded. Unlike `perf_budget!` it is
/// NEVER skipped: a hang guard must still fire on a loaded host, so it only
/// widens. Prints `deadline_budget: elapsed=… budget=…ms` to stderr on every
/// run and evaluates to `body`'s value.
///
/// `deadline_budget!(budget_ms, at_least = min_ms, { body })` also asserts the
/// body took AT LEAST `min_ms` (unscaled -- load only makes things slower), for
/// tests proving a call really waited out its declared timeout.
///
/// timing-helper
#[macro_export]
macro_rules! deadline_budget {
    ($budget_ms:expr, $body:block) => {
        $crate::deadline_budget!($budget_ms, at_least = 0, $body)
    };
    ($budget_ms:expr, at_least = $min_ms:expr, $body:block) => {{
        let __db_scale: u64 = if $crate::common::perf_skipped() { 3 } else { 1 };
        let __db_budget_ms: u64 = ($budget_ms as u64) * __db_scale;
        let __db_start = std::time::Instant::now();
        let __db_out = $body;
        let __db_elapsed = __db_start.elapsed();
        eprintln!("deadline_budget: elapsed={__db_elapsed:?} budget={__db_budget_ms}ms");
        assert!(
            __db_elapsed <= std::time::Duration::from_millis(__db_budget_ms),
            "deadline_budget: elapsed={__db_elapsed:?} budget={__db_budget_ms}ms"
        );
        assert!(
            __db_elapsed >= std::time::Duration::from_millis($min_ms as u64),
            "deadline_budget: elapsed={__db_elapsed:?} under minimum {}ms",
            $min_ms
        );
        __db_out
    }};
}

/// Readiness window for a spawned `mcphost serve` subprocess. 30 s normally,
/// 90 s when `MCPHOST_PERF_SKIP=1` marks the host as loaded (the same signal
/// `perf_budget!` honors), instead of a fixed 10 s that a loaded host misses.
/// timing-helper
pub fn serve_ready_timeout() -> std::time::Duration {
    if std::env::var("MCPHOST_PERF_SKIP").ok().as_deref() == Some("1") {
        std::time::Duration::from_secs(90)
    } else {
        std::time::Duration::from_secs(30)
    }
}

/// Failure-path diagnostics for a spawned child whose stderr is piped: kill
/// and reap the child FIRST, then drain stderr (EOF is guaranteed once the
/// child has exited). Reading a live child's stderr to EOF blocks forever
/// (gate hang 2026-10-09, runs 618 + 619).
pub fn kill_then_read_stderr(
    child: &mut std::process::Child,
    stderr: Option<std::process::ChildStderr>,
) -> String {
    use std::io::Read;
    let _ = child.kill();
    let _ = child.wait();
    let mut buf = String::new();
    if let Some(mut s) = stderr {
        let _ = s.read_to_string(&mut buf);
    }
    buf
}

// ---- PRD-mcphost-paged-trait-on-every-list-verb: recorded responses -------

/// Canonical contract verbs the recorder never calls: with no required
/// argument they would act on the recording tenant itself (offboard it,
/// rotate its key) or reach a payment provider.
const PGTR_NEVER_CALL: &[&str] = &[
    "host.self.offboard",
    "host.key.rotate",
    "host.oauth.revoke_all",
    "host.enduser.assertion_secret_rotate",
    "host.registry.publish",
    "billing.checkout",
    "host.export",
];

/// Minimal arguments for the list-shaped verbs that have a required
/// argument. `{channel}`/`{thread}` are filled from the seeded fixtures.
fn pgtr_seed_args(verb: &str, channel: &str, thread: &str) -> Value {
    match verb {
        "host.channel.read" => json!({"channel_id": channel}),
        "host.msg.thread" => json!({"thread_id": thread}),
        "host.docs.search" => json!({"query": "pgtr"}),
        "host.enduser.audit" => json!({"subject": "pgtr-subject"}),
        "host.state.query" => json!({"table": "pgtr_state"}),
        "host.table.query" => json!({"sql": "SELECT kind FROM pgtr_events"}),
        "host.table.next_questions" => json!({"table": "pgtr_events"}),
        "host.tool.logs" | "host.tool.history" => json!({"name": "pgtr_echo"}),
        "host.oauth.audit_export" => json!({"since": 0, "until": 4_102_444_800_i64}),
        _ => json!({}),
    }
}

/// Every canonical (non-alias) verb in the committed contract.
pub fn pgtr_canonical_contract_tools() -> Vec<Value> {
    let kinds = mcphost::kinds::KindRegistry::with_builtin();
    let contract = mcphost::api_contract::dump_contract(&kinds);
    contract["tools"]
        .as_array()
        .expect("contract tools")
        .iter()
        .filter(|t| {
            let n = t["name"].as_str().unwrap_or("");
            n.contains('.') && mcphost::tool_aliases::resolve(n).is_none()
        })
        .cloned()
        .collect()
}

/// Records the suite's responses: boots a server, seeds one tenant (a
/// document, a named channel with one post, a message thread from a second
/// tenant, a state table, a data table, a published tool), then calls every
/// canonical contract verb once and keeps the `structuredContent` of each
/// call that succeeded. Returns `(verb, response)` pairs in contract order.
pub async fn pgtr_record_responses() -> Vec<(String, Value)> {
    let server = TestServer::start_with_signup_rate_limit(50).await;
    let (_ns, key) = signup(&server.base_url, "Pgtr Recorder").await;
    let (ns_b, key_b) = signup(&server.base_url, "Pgtr Sender").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let client_b = McpClient::with_bearer(&server.base_url, &key_b);
    let me = {
        let who = extract_structured(&client.tools_call("host.whoami", json!({})).await.expect("whoami"));
        who["namespace"].as_str().unwrap_or_default().to_string()
    };
    let _ = ns_b;

    let _ = client.tools_call("host.docs.put", json!({"name": "pgtr-doc", "content": "pgtr"})).await;
    let opened = extract_structured(
        &client.tools_call("host.channel.open", json!({"name": "pgtr"})).await.expect("channel.open"),
    );
    let channel = opened["channel_id"].as_str().unwrap_or_default().to_string();
    let _ = client.tools_call("host.channel.post", json!({"channel": channel, "body": "hi"})).await;
    let sent = client_b.tools_call("host.msg.send", json!({"to": [me], "body": "hi"})).await;
    let thread = sent
        .map(|v| extract_structured(&v)["thread_id"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let _ = client
        .tools_call("host.state.table_create", json!({"name": "pgtr_state", "schema": {"id": "integer"}}))
        .await;
    let _ = client
        .tools_call("host.table.create", json!({"name": "pgtr_events", "columns": {"kind": "text"}}))
        .await;
    let _ = client.tools_call("host.table.append", json!({"table": "pgtr_events", "rows": [{"kind": "a"}]})).await;
    let _ = client
        .tools_call(
            "host.tool.publish",
            json!({"name": "pgtr_echo", "kind": "echo", "spec": {}}),
        )
        .await;

    let mut recorded = Vec::new();
    for tool in pgtr_canonical_contract_tools() {
        let verb = tool["name"].as_str().unwrap_or_default().to_string();
        if PGTR_NEVER_CALL.contains(&verb.as_str()) {
            continue;
        }
        let args = pgtr_seed_args(&verb, &channel, &thread);
        if let Ok(result) = client.tools_call(&verb, args).await {
            let structured = extract_structured(&result);
            if !structured.is_null() {
                recorded.push((verb, structured));
            }
        }
    }
    recorded
}

// ---- PRD-mcphost-declared-ordering-per-verb helpers -------------------------

/// Walk one paged verb to its end, following `next_cursor`, and return every
/// row's `id_key` value in the order the pages delivered them. `pace` is
/// slept between pages so a concurrent writer task gets scheduled in between.
#[allow(clippy::too_many_arguments)]
pub async fn vord_walk(
    client: &McpClient,
    verb: &str,
    array_key: &str,
    id_key: &str,
    base_args: Value,
    limit: u64,
    pace: std::time::Duration,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..1000 {
        let mut args = base_args.clone();
        args["limit"] = json!(limit);
        if let Some(c) = &cursor {
            args["cursor"] = json!(c);
        }
        let page = extract_structured(
            &client
                .tools_call(verb, args)
                .await
                .unwrap_or_else(|e| panic!("{verb}: {} {}", e.code, e.message)),
        );
        for row in page[array_key].as_array().unwrap_or_else(|| panic!("{verb}: no {array_key}: {page}")) {
            ids.push(row[id_key].as_str().unwrap_or_else(|| panic!("{verb}: no {id_key}: {row}")).to_string());
        }
        match page.get("next_cursor").and_then(Value::as_str) {
            Some(c) => cursor = Some(c.to_string()),
            None => return ids,
        }
        tokio::time::sleep(pace).await;
    }
    panic!("{verb}: walk did not terminate");
}

/// `(duplicate ids, pre-existing ids the walk never delivered)`.
pub fn vord_dups_and_skips(walked: &[String], pre_existing: &[String]) -> (usize, usize) {
    let mut seen = std::collections::HashSet::new();
    let dups = walked.iter().filter(|id| !seen.insert(id.as_str())).count();
    let skips = pre_existing.iter().filter(|id| !seen.contains(id.as_str())).count();
    (dups, skips)
}

/// Insert `total` finished runs for `tenant_id`, oldest first; the first
/// `fails` carry `progress.budget.verdict == "exceeded"`, the rest `"ok"`. Returns
/// the ids of the failing runs.
pub async fn vord_seed_runs(server: &TestServer, tenant_id: i64, total: usize, fails: usize) -> Vec<String> {
    let mut failing = Vec::new();
    for i in 0..total {
        let verdict = if i < fails { "exceeded" } else { "ok" };
        let id = server
            .state
            .db
            .insert_done_run_with_progress_for_test(
                tenant_id,
                "vord_echo".to_string(),
                json!({"budget": {"verdict": verdict}}).to_string(),
            )
            .await
            .expect("seed run");
        if i < fails {
            failing.push(id);
        }
    }
    failing
}

/// PRD-mcphost-time-to-first-success-public fixtures: inserts a tenant with
/// the given `origin`/`client_name`, born at `created`, and stamps each
/// milestone `stamps[i]` seconds after birth (`None` = unstamped), in
/// `ACTIVATION_MILESTONES` order.
#[allow(clippy::too_many_arguments)]
pub async fn ttfs_tenant(
    db: &Db,
    namespace: &str,
    origin: &str,
    client_name: Option<&str>,
    created: i64,
    stamps: [Option<i64>; 4],
) -> mcphost::db::Tenant {
    let tenant = db
        .create_tenant_attributed(
            namespace.to_string(),
            namespace.to_string(),
            format!("hash-{namespace}"),
            (origin == "synthetic").then(|| "fleet".to_string()),
            None,
            client_name.map(str::to_string),
            None,
            origin.to_string(),
            None,
        )
        .await
        .expect("create tenant");
    db.set_tenant_stamp_for_test(tenant.id, "created_unix", created).await.expect("created");
    for (column, stamp) in
        ["first_call_unix", "first_publish_unix", "first_own_call_unix", "second_session_unix"].into_iter().zip(stamps)
    {
        if let Some(after) = stamp {
            db.set_tenant_stamp_for_test(tenant.id, column, created + after).await.expect("stamp");
        }
    }
    db.find_tenant_by_namespace(namespace.to_string()).await.expect("query").expect("tenant exists")
}

/// PRD-mcphost-oauth-first-entry-url: DCR-register a native client, post the
/// consent form for `resource` with `consent_extra` (`new_workspace=1`, or a
/// `tenant_key`), and exchange the code. Returns `(client_id, token response
/// JSON)` -- the whole token response, so a caller reads `access_token`
/// without this helper deciding what else it needs.
pub async fn oauth_consent_and_token(base_url: &str, resource: &str, consent_extra: &[(&str, &str)]) -> (String, Value) {
    oauth_consent_and_token_scoped(base_url, resource, "mcp", consent_extra).await
}

/// [`oauth_consent_and_token`] for an explicit `scope` request instead of `mcp`.
pub async fn oauth_consent_and_token_scoped(
    base_url: &str,
    resource: &str,
    scope: &str,
    consent_extra: &[(&str, &str)],
) -> (String, Value) {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use sha2::{Digest, Sha256};

    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let register: Value = http
        .post(format!("{base_url}/oauth/register"))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("register json");
    let client_id = register["client_id"].as_str().expect("client_id").to_string();
    let verifier = "a-pkce-verifier-at-least-43-chars-long-for-realism";
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut form: Vec<(&str, &str)> = vec![
        ("response_type", "code"),
        ("client_id", client_id.as_str()),
        ("redirect_uri", "http://127.0.0.1/cb"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", "abc"),
        ("scope", scope),
        ("resource", resource),
    ];
    form.extend_from_slice(consent_extra);
    let consent = http
        .post(format!("{base_url}/oauth/authorize"))
        .form(&form)
        .send()
        .await
        .expect("POST /oauth/authorize");
    assert!(consent.status().is_redirection(), "consent must redirect: {}", consent.status());
    let location = consent.headers().get("location").expect("Location").to_str().unwrap().to_string();
    let (_, query) = location.split_once('?').expect("redirect query");
    let code = query.split('&').find_map(|p| p.strip_prefix("code=")).unwrap_or_else(|| panic!("no code in {location}"));
    let token: Value = http
        .post(format!("{base_url}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", "http://127.0.0.1/cb"),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("token json");
    (client_id, token)
}

/// The signature-unverified `aud` claim of a JWT access token, for tests
/// that assert which resource a token was minted for.
pub fn jwt_aud(token: &str) -> String {
    use base64::Engine as _;
    let payload = token.split('.').nth(1).expect("JWT payload segment");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).expect("payload base64url");
    let claims: Value = serde_json::from_slice(&bytes).expect("payload json");
    claims["aud"].as_str().expect("aud claim").to_string()
}

/// PRD-mcphost-skill-md-generated-from-plugin: a scratch copy of the files
/// `mcphost gen-docs` reads and writes (README.md, docs/, www/), so a test
/// can run the real binary without touching the worktree.
pub fn gen_docs_scratch_tree(label: &str) -> PathBuf {
    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let dest = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &dest);
            } else {
                std::fs::copy(e.path(), dest).unwrap();
            }
        }
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "mcphost-gendocs-{label}-{}-{}",
        process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR"));
    for f in ["README.md", mcphost::gendocs::INSTALL_DOC_PATH] {
        std::fs::copy(src.join(f), root.join(f)).unwrap();
    }
    copy_dir(&src.join("docs"), &root.join("docs"));
    copy_dir(&src.join("www"), &root.join("www"));
    root
}

/// Runs the built `mcphost gen-docs [args]` in `dir`.
pub fn run_gen_docs(dir: &Path, args: &[&str]) -> process::Output {
    process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("gen-docs")
        .args(args)
        .current_dir(dir)
        .env_remove("MCPHOST_SUPPORT_URL")
        .output()
        .expect("run mcphost gen-docs")
}

/// PRD-mcphost-push-from-directory: signs up a fresh tenant and returns its
/// personal `/u/<secret>/mcp` URL (what `mcphost push --url` takes).
pub async fn tenant_url(base_url: &str) -> String {
    let session = McpClient::new(base_url).with_session_continuity();
    session
        .tools_call("signup", json!({"name": "Push Tenant"}))
        .await
        .expect("signup");
    let rotated = session.tools_call("host.key_rotate", json!({})).await.expect("host.key_rotate");
    extract_structured(&rotated)["url"].as_str().expect("key_rotate url").to_string()
}

/// A scratch tool directory under the OS temp dir, removed on drop.
pub struct ToolDirFixture(pub PathBuf);

impl ToolDirFixture {
    /// Copies `examples/tools/<example>` into a fresh scratch directory.
    pub fn from_example(example: &str) -> Self {
        let scratch = TempDataDir::new();
        let dir = scratch.0.clone();
        std::mem::forget(scratch);
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/tools").join(example);
        copy_tree(&src, &dir);
        Self(dir)
    }

    pub fn write(&self, rel: &str, body: &str) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, body).expect("write fixture file");
    }
}

impl Drop for ToolDirFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read_dir").flatten() {
        let to = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), to).expect("copy");
        }
    }
}

/// One SSE event read off a `subscriptions/listen` stream.
#[derive(Debug, Clone, PartialEq)]
pub enum ListenFrame {
    /// A `data:` event carrying a JSON-RPC message.
    Message(Value),
    /// A `:` comment line (rmcp's keep-alive).
    Comment(String),
}

/// PRD-mcphost-subscriptions-listen: a raw streaming client for one
/// `subscriptions/listen` POST through the prod transport (`build_router`).
pub struct ListenStream {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub request_id: u64,
    resp: Option<reqwest::Response>,
    buf: String,
}

impl ListenStream {
    /// Open a listen for `filter` (e.g. `{"toolsListChanged": true}`) as
    /// request `request_id`; `key` goes in the Authorization header when set.
    pub async fn open_with(base_url: &str, key: Option<&str>, request_id: u64, filter: Value) -> Self {
        let body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "subscriptions/listen",
            "params": {
                "notifications": filter,
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                    "io.modelcontextprotocol/clientInfo": {"name": "mcphost-test", "version": "0.1.0"},
                },
            },
        });
        let mut req = reqwest::Client::new()
            .post(format!("{base_url}/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "subscriptions/listen")
            .json(&body);
        if let Some(key) = key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.send().await.expect("send subscriptions/listen");
        Self { status: resp.status().as_u16(), headers: resp.headers().clone(), request_id, resp: Some(resp), buf: String::new() }
    }

    /// [`Self::open_with`] for tools and prompts list changes.
    pub async fn open(base_url: &str, key: &str, request_id: u64) -> Self {
        Self::open_with(base_url, Some(key), request_id, json!({"toolsListChanged": true, "promptsListChanged": true})).await
    }

    /// The next event within `within`, or `None` on timeout or end of stream.
    pub async fn next_frame(&mut self, within: std::time::Duration) -> Option<ListenFrame> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..end + 2).collect();
                let block = block.replace("\r\n", "\n");
                let data: Vec<&str> = block
                    .lines()
                    .filter_map(|l| l.strip_prefix("data: ").or_else(|| l.strip_prefix("data:")))
                    .collect();
                let data = data.join("\n");
                if !data.trim().is_empty() {
                    let msg = serde_json::from_str(&data).unwrap_or_else(|e| panic!("listen frame is not JSON: {e}: {data}"));
                    return Some(ListenFrame::Message(msg));
                }
                if let Some(c) = block.lines().find_map(|l| l.strip_prefix(':')) {
                    return Some(ListenFrame::Comment(c.trim().to_string()));
                }
                continue; // an empty priming event
            }
            let resp = self.resp.as_mut()?;
            match tokio::time::timeout_at(deadline, resp.chunk()).await {
                Ok(Ok(Some(chunk))) => self.buf.push_str(&String::from_utf8_lossy(&chunk)),
                Ok(Ok(None)) | Ok(Err(_)) => {
                    self.resp = None;
                    if self.buf.trim().is_empty() {
                        return None;
                    }
                    self.buf.push_str("\n\n");
                }
                Err(_) => return None,
            }
        }
    }

    /// Next JSON-RPC message, skipping keep-alive comments.
    pub async fn next_message(&mut self, within: std::time::Duration) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.next_frame(left).await? {
                ListenFrame::Message(m) => return Some(m),
                ListenFrame::Comment(_) => continue,
            }
        }
    }

    /// The next `notifications/*` message's method, skipping keep-alives.
    pub async fn next_method(&mut self, within: std::time::Duration) -> Option<(String, Value)> {
        let msg = self.next_message(within).await?;
        Some((msg.get("method").and_then(Value::as_str).unwrap_or_default().to_string(), msg))
    }

    /// Close the stream from the client side.
    pub fn close(&mut self) {
        self.resp = None;
    }
}
