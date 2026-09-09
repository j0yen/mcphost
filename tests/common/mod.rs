//! Shared integration-test harness: spin up a real `mcphost` server on an
//! ephemeral port against a fresh temp `$MCPHOST_DATA_DIR`, and a tiny
//! JSON-RPC-over-streamable-HTTP client to drive it.
//!
//! This module is `mod`-included separately into every `tests/ac*.rs`
//! binary (the standard Rust integration-test pattern), so any one binary
//! uses only a subset of it — `dead_code` is expected, not a real
//! findings.
#![allow(dead_code)]

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

pub struct TestServer {
    pub base_url: String,
    pub data_dir: TempDataDir,
    pub state: Arc<AppState>,
}

impl TestServer {
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

    /// The one real constructor every `start_*` helper above funnels
    /// into -- billing config and client are the two fields
    /// PRD-grand-loop-billing added to `AppState`; every other field is
    /// unchanged from before this PRD.
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
        let data_dir = TempDataDir::new();
        let db = Db::open(&data_dir.0).expect("open db");
        db.migrate().await.expect("migrate");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let base_url = format!("http://{addr}");

        let state = Arc::new(AppState {
            db,
            kinds,
            secrets: SecretBox::from_passphrase("test-secret-key"),
            admin_key,
            public_url: base_url.clone(),
            call_timeout,
            registry,
            http_client: reqwest::Client::new(),
            sandbox_mechanism: None,
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
        });

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
            bearer: None,
            next_id: AtomicU64::new(1),
            client_name: "mcphost-test".to_string(),
            client_version: "0.1.0".to_string(),
        }
    }

    pub fn with_bearer(base_url: &str, key: &str) -> Self {
        let mut c = Self::new(base_url);
        c.bearer = Some(key.to_string());
        c
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
        self.post_with_extra(body, mcp_protocol_version_header, mcp_name_override, None)
            .await
    }

    /// Same as [`Self::post_with`], plus one arbitrary extra header --
    /// PRD-mcphost-synthetic-flag's `x-mcphost-synthetic` is the only user
    /// today (see [`Self::tools_call_with_header`]), kept generic rather
    /// than hardcoding that name so a future header-driven AC doesn't need
    /// its own copy of this method.
    async fn post_with_extra(
        &self,
        body: &Value,
        mcp_protocol_version_header: Option<&str>,
        mcp_name_override: Option<&str>,
        extra_header: Option<(&str, &str)>,
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
            .post(format!("{}/mcp", self.base_url))
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
        if let Some((name, value)) = extra_header {
            req = req.header(name, value);
        }
        req.send().await.expect("send request")
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
        self.call_with_extra_header(method, params, None).await
    }

    async fn call_with_extra_header(
        &self,
        method: &str,
        mut params: Value,
        extra_header: Option<(&str, &str)>,
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
            .post_with_extra(&body, Some("2026-07-28"), None, extra_header)
            .await;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .unwrap_or_else(|e| panic!("parse {method} response ({status}): {e}"));
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
        Ok(body.get("result").cloned().unwrap_or(Value::Null))
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
        self.call_with_extra_header(
            "tools/call",
            json!({"name": name, "arguments": arguments}),
            Some(header),
        )
        .await
    }
}

/// PRD-mcphost-composition `compose_ac*.rs` suite: `host.tool_publish` a
/// tool of `kind` under local `name`, returning the qualified
/// `<namespace>.<name>` `tools/call` would use. Panics on a publish
/// failure (every caller in that suite expects success; a test asserting a
/// publish-time rejection calls `host.tool_publish` directly instead).
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
            .record_call(tenant_id, tool_name.to_string(), 1, true, None, None, None, "ok")
            .await
            .expect("record_call");
    }
}

/// `CallToolResult::structured` puts the value in `structuredContent`;
/// fall back to parsing the first text content block for safety.
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
