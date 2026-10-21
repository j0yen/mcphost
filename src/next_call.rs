//! PRD-mcphost-onboarding-dead-ends-zero: `AppError::next_call` -- the one
//! exhaustive table that gives every error the tool and arguments to call
//! next, injected at the single envelope point in `errors.rs`.

use crate::error_registry::ErrorClass;
use crate::errors::AppError;
use serde_json::{Map, Value, json};

/// The next step an agent should take after an error: a tool name and the
/// arguments to pass it. `retry_after_s` is set only for the waits
/// (`rate_limited`, `internal`, `storage`).
#[derive(Debug, Clone, PartialEq)]
pub struct NextCall {
    pub tool: &'static str,
    pub args: Value,
    pub retry_after_s: Option<u64>,
}

impl NextCall {
    fn new(tool: &'static str, args: Value) -> Self {
        Self { tool, args, retry_after_s: None }
    }

    fn wait(tool: &'static str, retry_after_s: u64) -> Self {
        Self { tool, args: json!({}), retry_after_s: Some(retry_after_s) }
    }

    pub fn to_json(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("tool".to_string(), json!(self.tool));
        obj.insert("args".to_string(), self.args.clone());
        if let Some(s) = self.retry_after_s {
            obj.insert("retry_after_s".to_string(), json!(s));
        }
        Value::Object(obj)
    }
}

/// The name a caller uses for `tool`: its own namespace prefix stripped, a
/// shared tool's owner prefix kept. The one rule both the real call
/// (`handler.rs` `ran_tool`) and the dry-run verdict (`host.tool_call
/// dry_run`) name the tool by, so their `data.tool` and `next_call` agree.
pub fn caller_tool_name<'a>(namespace: &str, tool: &'a str) -> &'a str {
    match tool.split_once('.') {
        Some((ns, local)) if !namespace.is_empty() && ns == namespace => local,
        _ => tool,
    }
}

/// What an arm of the table may need beyond the error itself. Every field is
/// optional: an arm with nothing to fill in uses its context-free form.
#[derive(Debug, Clone, Default)]
pub struct ErrorCtx {
    /// The tool the failing call named.
    pub tool: Option<String>,
    /// A registered kind name (first of `KindRegistry::names()`).
    pub kind: Option<String>,
    /// The offending top-level argument and the value the caller sent
    /// (JSON text, possibly an excerpt).
    pub argument: Option<String>,
    pub got: Option<String>,
    pub expected_type: Option<String>,
    /// `secret.<name>` names the tool's spec references.
    pub secrets: Vec<String>,
    /// The plan ceiling that tripped.
    pub plan_ceiling: Option<String>,
    pub retry_after_s: Option<u64>,
}

impl ErrorCtx {
    /// The context an error carries in its own `data` (what `with_tool` and
    /// `describe_args_error` already put there), so the envelope point needs
    /// no call-site help.
    pub fn from_error(err: &AppError) -> Self {
        let mut ctx = Self::default();
        if let AppError::ToolNotFound(name) = err {
            ctx.tool = Some(name.clone());
        }
        if let AppError::UnknownKind { registered, .. } = err {
            ctx.kind = registered.first().map(|k| k.to_string());
        }
        if let AppError::Structured { data: Value::Object(d), .. } = err {
            let text = |k: &str| d.get(k).and_then(Value::as_str).map(str::to_string);
            ctx.tool = text("tool");
            ctx.argument = text("argument");
            ctx.got = text("got");
            ctx.expected_type = text("expected_type");
            ctx.plan_ceiling = text("ceiling");
            ctx.retry_after_s = d.get("retry_after_s").and_then(Value::as_u64);
            if let Some(Value::Array(names)) = d.get("secrets") {
                ctx.secrets = names.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
            }
        }
        ctx
    }
}

/// A valid example of a JSON-Schema type, for when the offending value is
/// unusable (truncated excerpt, missing).
fn example_of(expected: &str) -> Value {
    match expected {
        "integer" => json!(1),
        "number" => json!(1.5),
        "boolean" => json!(true),
        "array" => json!([]),
        "object" => json!({}),
        _ => json!("example"),
    }
}

/// The offending value coerced to `expected` (`4.78` -> `4` for integer), or
/// an example of the type when it cannot be.
fn coerce(got: Option<&str>, expected: &str) -> Value {
    let parsed: Option<Value> = got.and_then(|g| serde_json::from_str(g).ok());
    let first = expected.split(" or ").next().unwrap_or(expected);
    match (first, parsed) {
        ("integer", Some(Value::Number(n))) => n
            .as_f64()
            .map(|f| json!(f.trunc() as i64))
            .unwrap_or_else(|| example_of(first)),
        ("integer", Some(Value::String(s))) => s
            .trim()
            .parse::<f64>()
            .map(|f| json!(f.trunc() as i64))
            .unwrap_or_else(|_| example_of(first)),
        ("number", Some(Value::String(s))) => {
            s.trim().parse::<f64>().map(|f| json!(f)).unwrap_or_else(|_| example_of(first))
        }
        ("string", Some(Value::Number(n))) => json!(n.to_string()),
        ("string", Some(Value::Bool(b))) => json!(b.to_string()),
        ("boolean", Some(Value::String(s))) => match s.as_str() {
            "true" => json!(true),
            "false" => json!(false),
            _ => example_of(first),
        },
        _ => example_of(first),
    }
}

fn quickstart(ctx: &ErrorCtx) -> NextCall {
    let args = match &ctx.kind {
        Some(kind) => json!({"kind": kind}),
        None => json!({}),
    };
    NextCall::new("host.quickstart", args)
}

fn args_invalid(ctx: &ErrorCtx) -> NextCall {
    let (Some(tool), Some(arg)) = (&ctx.tool, &ctx.argument) else {
        return NextCall::new("host.tool.list", json!({}));
    };
    let value = coerce(ctx.got.as_deref(), ctx.expected_type.as_deref().unwrap_or("string"));
    NextCall::new("host.tool_call", json!({"name": tool, "args": {arg: value}}))
}

fn upstream(code: &str, ctx: &ErrorCtx) -> NextCall {
    match (code, ctx.secrets.first()) {
        ("upstream_auth", Some(name)) => NextCall::new("host.secret.set", json!({"name": name})),
        (_, _) => match &ctx.tool {
            Some(tool) => NextCall::new("host.tool_test", json!({"name": tool})),
            None => NextCall::new("host.tool_test", json!({})),
        },
    }
}

/// The table for a `Structured` code, keyed by the code string; a code with
/// no row of its own falls back on its registry class.
fn by_code(code: &str, ctx: &ErrorCtx) -> NextCall {
    match code {
        "args_invalid" => args_invalid(ctx),
        "tool_exception" => match &ctx.tool {
            Some(tool) => NextCall::new("host.tool.logs", json!({"name": tool})),
            None => NextCall::new("host.call_trace", json!({})),
        },
        "host_not_allowed" => NextCall::new("host.tool_publish", json!({})),
        "unknown_argument" => NextCall::new("host.help.search", json!({})),
        "end_user_required" | "end_user_explicit_forbidden" => {
            NextCall::new("host.enduser.whoami", json!({}))
        }
        "signup_rate_limited" => match ctx.retry_after_s {
            Some(s) => NextCall::new("host.quickstart", json!({"retry_after_s": s})),
            None => quickstart(ctx),
        },
        "rate_limited" => NextCall::wait("host.whoami", ctx.retry_after_s.unwrap_or(5)),
        "plan_limit" => NextCall::new("billing.plans", json!({})),
        c if c == "upstream_status" || c.starts_with("upstream_") => upstream(c, ctx),
        c => match crate::error_registry::lookup(c).map(|row| row.class) {
            Some(ErrorClass::PlanLimit) => NextCall::new("billing.plans", json!({})),
            Some(ErrorClass::HostFault) => NextCall::wait("host.whoami", 5),
            _ => quickstart(ctx),
        },
    }
}

impl AppError {
    /// The tool and arguments to call after this error. Exhaustive over
    /// every variant (no `_` arm): a new variant does not compile until it
    /// has a row here.
    pub fn next_call(&self, ctx: &ErrorCtx) -> NextCall {
        match self {
            AppError::Unauthorized
            | AppError::TenantKeyMissing(_)
            | AppError::TenantKeyInvalid
            | AppError::TenantNotFound(_)
            | AppError::InvalidParams(_)
            | AppError::InvalidToolName(_)
            | AppError::SpecTooLarge(_)
            | AppError::RegistryDisabled => quickstart(ctx),
            AppError::UnknownKind { registered, .. } => {
                let first = ctx.kind.clone().or_else(|| registered.first().map(|k| k.to_string()));
                quickstart(&ErrorCtx { kind: first, ..ctx.clone() })
            }
            AppError::Forbidden
            | AppError::TenantDisabled { .. }
            | AppError::NamespaceUnverified
            | AppError::HandoffTokenInvalid
            | AppError::HandoffTokenRedeemed
            | AppError::HandoffTokenExpired
            | AppError::InvalidToken(_)
            | AppError::ConflictingCredentials
            | AppError::WrongTenant
            | AppError::InsufficientScope
            | AppError::AuthConflict => NextCall::new("host.whoami", json!({})),
            AppError::RateLimited => NextCall::wait("host.whoami", ctx.retry_after_s.unwrap_or(5)),
            AppError::ToolNotFound(_) | AppError::TooManyTools(_) => {
                NextCall::new("host.tool.list", json!({}))
            }
            AppError::InvalidSpec(_) | AppError::CallTimeout(_) | AppError::RegistryRejected(_) => {
                NextCall::new("host.tool_test", json!({}))
            }
            AppError::InvalidArgs(_) => args_invalid(ctx),
            AppError::VersionNotFound { .. } => NextCall::new("host.tool.history", json!({})),
            AppError::SecretMissing(name) => NextCall::new("host.secret.set", json!({"name": name})),
            AppError::Structured { code, .. } => by_code(code, ctx),
            AppError::MultiInvalid { errors, .. } => match errors.first() {
                Some(first) => first.next_call(ctx),
                None => NextCall::new("host.tool_test", json!({})),
            },
            AppError::Storage(_) | AppError::Internal(_) => NextCall::wait("host.whoami", 5),
            AppError::ShareQuotaExceeded { .. }
            | AppError::IssuerQuotaExceeded { .. }
            | AppError::TrustedIssuerQuotaExceeded { .. } => NextCall::new("billing.plans", json!({})),
            AppError::GroupNotFound(_) => NextCall::new("host.group.list", json!({})),
            AppError::IssuerAlreadyRegistered | AppError::IssuerNotFound(_) => {
                NextCall::new("host.oauth.issuers", json!({}))
            }
        }
    }
}
