//! Tool execution kinds: the `Kind` trait and its registry.
//!
//! `mcphost` publishes tools of a *kind* (`echo` is the reference
//! implementation shipped here; REST-wrapper and code kinds are separate
//! PRDs that extend this crate's registry via `build_into`). A `Kind` knows
//! how to validate a tenant-supplied `spec`, describe the tool it produces
//! for `tools/list`, and execute a call against it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

pub mod conformance;
pub mod echo;
pub mod http;
pub mod infer;
pub mod python;

/// Error returned by a [`Kind`]'s methods. Distinct from the JSON-RPC error
/// the host ultimately answers with; `mcphost::errors` maps this onto that.
#[derive(Debug, thiserror::Error, Clone)]
pub enum KindError {
    #[error("invalid spec: {0}")]
    InvalidSpec(String),
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("execution failed: {0}")]
    Exec(String),
    /// A kind-specific structured error carrying a stable, machine-readable
    /// `code` its own PRD/acceptance criteria name directly (e.g.
    /// `host_not_allowed`, `upstream_status`, `template_error`) rather than
    /// this crate's generic `invalid_spec`/`invalid_args`/`internal`
    /// taxonomy. `data` is merged into the JSON-RPC error's `data` field
    /// (alongside `error_code`) so a caller can match extra fields like
    /// `retry_after_s` or `status` without parsing prose. The `http` kind
    /// (`mcphost::kinds::http`) is this variant's first user.
    #[error("{message}")]
    Structured {
        code: &'static str,
        message: String,
        data: Value,
    },
}

impl KindError {
    /// A [`KindError::Structured`] with no extra `data` fields beyond
    /// `error_code`.
    pub fn structured(code: &'static str, message: impl Into<String>) -> Self {
        Self::Structured {
            code,
            message: message.into(),
            data: Value::Null,
        }
    }

    /// A [`KindError::Structured`] carrying extra `data` fields (must be a
    /// JSON object; merged into the error's `data` alongside `error_code`).
    pub fn structured_with(code: &'static str, message: impl Into<String>, data: Value) -> Self {
        Self::Structured {
            code,
            message: message.into(),
            data,
        }
    }
}

/// What a `Kind::describe` call reports about the tool it would publish.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A complete, minimal, working example for this kind (PRD-mcphost-publish-first-try
/// requirement 1 / 4): a `spec` that `host.tool_publish` accepts as-is, `call_args`
/// that satisfy the resulting tool's own `describe`d schema, and a one-sentence
/// `blurb` naming which fields are optional and why. `host.tool_publish`'s wire
/// description (`handler::host_tools`) and `host.quickstart` are both built from
/// this single source per kind, so they cannot drift from each other -- though
/// (see the PRD's requirement 6, not attempted this tick) the crate's README is
/// still hand-maintained separately, not rendered from this same source.
#[derive(Debug, Clone)]
pub struct KindExample {
    pub spec: Value,
    pub call_args: Value,
    pub blurb: &'static str,
}

/// Resolves one of the calling tenant's stored secrets by name.
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, name: &str) -> Option<String>;
}

/// A resolver with no secrets, for contexts (like the conformance suite)
/// that don't need one.
pub struct NoSecrets;
impl SecretResolver for NoSecrets {
    fn resolve(&self, _name: &str) -> Option<String> {
        None
    }
}

/// Handle a `Kind::call` uses to append lines to that call's log, later
/// readable via `host.tool_logs`.
pub trait CallLog: Send + Sync {
    fn log(&self, line: &str);
}

/// A call log that discards everything, for contexts that don't need one.
pub struct NullLog;
impl CallLog for NullLog {
    fn log(&self, _line: &str) {}
}

/// Sink a [`Kind::call`] reports a sandboxed subprocess's resource usage to
/// (PRD-mcphost-code-tools requirement 8: the `calls` row records CPU time
/// and peak memory). A `Kind` with no subprocess to meter (`echo`, `http`)
/// never calls this; `handler.rs` reads back whatever was recorded (if
/// anything) after `Kind::call` returns and writes it into the `calls` row
/// alongside the rest of the call's metering.
pub trait ResourceSink: Send + Sync {
    fn record(&self, cpu_ms: i64, peak_rss_kb: i64);
}

/// A resource sink that discards everything, for contexts (tests, kinds
/// with no subprocess) that don't need one.
pub struct NullResourceSink;
impl ResourceSink for NullResourceSink {
    fn record(&self, _cpu_ms: i64, _peak_rss_kb: i64) {}
}

/// Context passed to every `Kind::call`: who is calling, how to reach their
/// secrets, when to give up, and where to log.
pub struct CallCtx {
    pub tenant_id: i64,
    pub namespace: String,
    pub secrets: Arc<dyn SecretResolver>,
    pub deadline: Instant,
    pub log: Arc<dyn CallLog>,
    /// Set by `host.tool_test` (P1 requirement 9): the call still hits the
    /// real upstream, but a `Kind` that supports test mode should include
    /// the rendered request (secrets redacted) in its result. Kinds that
    /// don't have a notion of a "rendered request" (e.g. `echo`) ignore
    /// this; it defaults to `false` for every ordinary call.
    pub test_mode: bool,
    /// See [`ResourceSink`]. Defaults to [`NullResourceSink`] everywhere but
    /// `handler.rs`'s real dispatch path.
    pub resources: Arc<dyn ResourceSink>,
}

impl CallCtx {
    /// Convenience constructor for tests and the conformance suite.
    pub fn for_test(tenant_id: i64, namespace: impl Into<String>) -> Self {
        Self {
            tenant_id,
            namespace: namespace.into(),
            secrets: Arc::new(NoSecrets),
            deadline: Instant::now() + Duration::from_secs(30),
            log: Arc::new(NullLog),
            test_mode: false,
            resources: Arc::new(NullResourceSink),
        }
    }

    pub fn time_remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// A tool execution kind. Implementors are registered in a [`KindRegistry`]
/// under a fixed [`Kind::name`]; `host.tool_publish` names the kind by that
/// string and stores the caller's `spec` alongside it.
#[async_trait::async_trait]
pub trait Kind: Send + Sync {
    /// The registry key this kind is published under (e.g. `"echo"`).
    fn name(&self) -> &'static str;

    /// Validate a tenant-supplied `spec` before it is stored. Return
    /// [`KindError::InvalidSpec`] naming what's wrong.
    fn validate(&self, spec: &Value) -> Result<(), KindError>;

    /// Deeper validation that needs to run out-of-process (PRD-mcphost-code-tools
    /// requirement 2: a `python` tool's source must be parsed and checked
    /// for `main` "in the sandbox, never in the host process", which means
    /// a subprocess call -- something [`Kind::validate`]'s synchronous
    /// signature can't do). Runs after `validate` succeeds, still before the
    /// spec is stored. Kinds with no such need (`echo`, `http`) use the
    /// default no-op.
    async fn validate_async(&self, _spec: &Value) -> Result<(), KindError> {
        Ok(())
    }

    /// Describe the tool this `spec` would publish: its (kind-local) name,
    /// description, and JSON Schema for arguments. The host overrides
    /// `ToolDescriptor::name` with `<namespace>.<published-name>` when it
    /// builds the wire-level `Tool`; what this method returns for `name` is
    /// only ever used as a hint (e.g. in error messages).
    fn describe(&self, spec: &Value) -> ToolDescriptor;

    /// Names of `secret.<name>` references this `spec` would need at call
    /// time, so `host.tool_publish` can reject an unknown secret before the
    /// tool is ever called (requirement 3 / AC3: `secret_missing` naming
    /// the secret). Kinds with no secret-templating concept (`echo`) don't
    /// override this; the default is "none needed."
    fn referenced_secrets(&self, _spec: &Value) -> Vec<String> {
        Vec::new()
    }

    /// A complete minimal working example for this kind (see
    /// [`KindExample`]), used to build `host.tool_publish`'s per-kind
    /// description and `host.quickstart`'s filled-in call sequence.
    /// Defaults to an empty placeholder so test-only `Kind` impls (e.g.
    /// `tests/ac15_call_timeout.rs`'s never-completing kind,
    /// `tests/ac17_kind_conformance.rs`'s conformance fixtures) don't have
    /// to implement it; every kind actually registered in `main.rs`
    /// (`echo`, `http`, `python`) overrides it.
    fn example(&self) -> KindExample {
        KindExample {
            spec: Value::Null,
            call_args: serde_json::json!({}),
            blurb: "",
        }
    }

    /// Execute a call. `args` have not yet been validated against the
    /// descriptor's input schema when this is invoked directly (the host's
    /// dispatch path validates first); implementations that are exercised
    /// directly by the conformance suite should still validate defensively.
    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError>;
}

/// Registry of known [`Kind`]s, keyed by [`Kind::name`]. `host.tool_publish`
/// looks kinds up here; an unregistered kind fails naming the registered
/// set.
#[derive(Clone, Default)]
pub struct KindRegistry {
    kinds: BTreeMap<&'static str, Arc<dyn Kind>>,
}

impl KindRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry shipped by this crate: `echo` only. Feature-PRD crates
    /// extend this by calling [`KindRegistry::register`] with their own
    /// kinds after constructing this base.
    pub fn with_builtin() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(echo::EchoKind));
        registry
    }

    pub fn register(&mut self, kind: Arc<dyn Kind>) -> &mut Self {
        self.kinds.insert(kind.name(), kind);
        self
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Kind>> {
        self.kinds.get(name).cloned()
    }

    /// Registered kind names, stable order, for error messages.
    pub fn names(&self) -> Vec<&'static str> {
        self.kinds.keys().copied().collect()
    }
}
