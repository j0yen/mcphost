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
}

/// What a `Kind::describe` call reports about the tool it would publish.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
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

/// Context passed to every `Kind::call`: who is calling, how to reach their
/// secrets, when to give up, and where to log.
pub struct CallCtx {
    pub tenant_id: i64,
    pub namespace: String,
    pub secrets: Arc<dyn SecretResolver>,
    pub deadline: Instant,
    pub log: Arc<dyn CallLog>,
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

    /// Describe the tool this `spec` would publish: its (kind-local) name,
    /// description, and JSON Schema for arguments. The host overrides
    /// `ToolDescriptor::name` with `<namespace>.<published-name>` when it
    /// builds the wire-level `Tool`; what this method returns for `name` is
    /// only ever used as a hint (e.g. in error messages).
    fn describe(&self, spec: &Value) -> ToolDescriptor;

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
