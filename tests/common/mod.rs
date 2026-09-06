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
        }
    }

    pub fn with_bearer(base_url: &str, key: &str) -> Self {
        let mut c = Self::new(base_url);
        c.bearer = Some(key.to_string());
        c
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
                    "clientInfo": {"name": "mcphost-test", "version": "0.1.0"}
                }
            }))
            .await;
        let body: Value = resp.json().await.expect("parse initialize response");
        body
    }

    async fn call(&self, method: &str, mut params: Value) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // Stateless 2026-07-28 requests carry the client context SEP-2575
        // requires on every request (no session to remember it from
        // `initialize`), not just in `initialize` itself.
        if let Some(obj) = params.as_object_mut() {
            obj.insert(
                "_meta".to_string(),
                json!({
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {},
                }),
            );
        }
        let resp = self
            .post(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }))
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

/// Polls `tools_call(qualified_name, args)` until it stops returning
/// `tool_building`, or `timeout` elapses. `kinds::python`'s first call to a
/// tool with no ready environment kicks off a background build and returns
/// `tool_building` immediately (see that module's docs); every
/// `python_ac*.rs` test that needs the tool to actually *run* polls through
/// that state with this helper rather than sleeping a fixed guess.
pub async fn poll_until_ready(
    client: &McpClient,
    qualified_name: &str,
    args: Value,
    timeout: std::time::Duration,
) -> Result<Value, RpcError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let result = client.tools_call(qualified_name, args.clone()).await;
        let is_building =
            matches!(&result, Err(e) if e.error_code.as_deref() == Some("tool_building"));
        if !is_building || std::time::Instant::now() >= deadline {
            return result;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
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
