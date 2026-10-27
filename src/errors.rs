//! `mcphost`'s own error taxonomy, and its mapping onto MCP JSON-RPC errors.
//!
//! Every acceptance criterion that names an error code (`tenant_disabled`,
//! `tool_not_found`, `rate_limited`, ...) gets it from [`AppError::code`],
//! carried in the JSON-RPC error's `data.error_code` field so a caller (or a
//! test) can match on it without parsing prose.

use rand::RngCore;
use rmcp::model::{ErrorCode, ErrorData};
use serde_json::{Map, Value, json};

use crate::kinds::KindError;
use crate::kinds::infer::KindSignal;

/// A short random correlation id for an `AppError::Internal` response
/// (audit finding 2, mcphost-polish-p0-20260930): the client only ever
/// sees "internal error; request_id=<id>"; the raw upstream detail
/// (Stripe API bodies, `io::Error`/pkcs8 decode text, ...) goes to the
/// server log at `error!` under the same id, so an operator can find the
/// real cause from the id a caller reports without the client ever seeing
/// that text. Same hex-encoding convention as `auth::generate_key`, just
/// shorter -- this is a log-correlation token, not a secret.
/// PRD-mcphost-error-envelope-schema-contract R1: the ONE constructor of an
/// error envelope (`code`, `message`, `data`; `data.error_code` names the
/// failure, an optional `data.retryable` says whether a retry can help).
/// `data` is always a JSON object: an object stays as it is, `null` becomes
/// `{}`, and any other value is kept as `{"detail": <value>}`, so a stored or
/// hand-built `null` can never reach a client that was promised an object.
pub fn error_envelope(code: ErrorCode, message: impl Into<String>, data: Value) -> ErrorData {
    let data = match data {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        other => {
            let mut map = Map::new();
            map.insert("detail".to_string(), other);
            map
        }
    };
    ErrorData::new(code, message.into(), Some(Value::Object(data)))
}

/// Name of the shared `$defs` entry every registered output schema carries.
pub const ERROR_ENVELOPE_DEF: &str = "ErrorEnvelope";

/// The JSON Schema of [`error_envelope`]'s result, generated here so the
/// output-schema registry restates nothing per verb.
pub fn error_envelope_schema() -> Value {
    json!({
        "type": "object",
        "required": ["code", "message", "data"],
        "properties": {
            "code": {"type": "integer"},
            "message": {"type": "string"},
            "data": {
                "type": "object",
                "required": ["error_code"],
                "properties": {
                    "error_code": {"type": "string"},
                    "retryable": {"type": "boolean"}
                }
            }
        }
    })
}

fn generate_request_id() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// A corrected example value for a well-known spec/argument field name,
/// shared by every kind's rejections (PRD-mcphost-publish-first-try
/// requirement 2, AC2). Keyed by the bare field name a `KindError`'s
/// "<field>: <description>" message convention already exposes (see
/// [`AppError::split_field`]) -- adding a kind's own field here is the only
/// step needed for its rejections to start carrying a real, resubmittable
/// `example`, no call-site changes required.
fn field_example(field: &str) -> Option<Value> {
    Some(match field {
        "name" => json!("my_tool"),
        "kind" => json!("echo"),
        "spec.schema" | "schema" => json!({
            "type": "object",
            "properties": {"msg": {"type": "string"}},
            "required": ["msg"],
        }),
        "method" => json!("GET"),
        "url" => json!("https://api.example.com/items/{{id}}"),
        "timeout_s" => json!(10),
        "response" => json!("json"),
        "body" => json!({"type": "object"}),
        "args_schema" => json!({"type": "object", "properties": {}}),
        "source" => json!("def main(args):\n    return {\"ok\": True}\n"),
        "requirements" => json!(["requests"]),
        "memory_mb" => json!(256),
        "network" => json!("none"),
        // PRD-mcphost-tenant-key-missing-is-invalid-params requirement 5
        // (AC6-7): a real tenant_key is a 64-char lowercase-hex string
        // (auth::generate_key) with no prefix -- this placeholder is
        // deliberately shaped so it can never be mistaken for (or collide
        // with) a real one, and never resolves to a tenant (AC7 proves it
        // by using it, not by string-matching it).
        "tenant_key" => json!("tk_example_REPLACE_WITH_THE_KEY_SIGNUP_RETURNED"),
        _ => return None,
    })
}

/// PRD-mcphost-unknown-kind-routes-to-recipe requirement 4: curated
/// synonyms for a genuinely-unknown `kind` whose vocabulary is common
/// enough to deserve a direct pointer even though its edit distance to
/// every registered kind/alias is nowhere near [`MAX_DID_YOU_MEAN_DISTANCE`]
/// -- `lambda`/`function`/`serverless`/`faas` all mean "run my code",
/// which this host already does as the `python` kind. Checked before the
/// edit-distance pass below, which stays the catch-all for an actual typo
/// (`"pytho"`, `"htpp"`, ...).
const DID_YOU_MEAN_HINTS: &[(&str, &str)] = &[
    ("lambda", "python"),
    ("function", "python"),
    ("functions", "python"),
    ("serverless", "python"),
    ("faas", "python"),
];

/// Requirement 4: "edit distance ≤ 2 against kinds and aliases" -- the
/// threshold a typo'd request (`"pythom"`, `"htpp"`) is matched under.
const MAX_DID_YOU_MEAN_DISTANCE: usize = 2;

/// Requirement 4: "max 3" -- `did_you_mean`'s own cap, hints included.
const MAX_DID_YOU_MEAN: usize = 3;

/// Plain Levenshtein distance (insert/delete/substitute, unit cost), byte-
/// wise -- every candidate this compares against (`registered`/`aliases`)
/// is ASCII, so byte-wise is character-wise here. `pub(crate)`:
/// PRD-mcphost-spec-unknown-field-rejection's `kinds::nearest_known_field`
/// reuses this same distance function for a spec field's `did_you_mean`
/// rather than a second hand-copied implementation.
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<u8> = a.bytes().collect();
    let b: Vec<u8> = b.bytes().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// `UnknownKind`'s `data.did_you_mean` (requirement 4, AC4): a curated
/// hint (see [`DID_YOU_MEAN_HINTS`]) first, then the closest registered
/// kinds/aliases within [`MAX_DID_YOU_MEAN_DISTANCE`] edits, nearest
/// first, capped at [`MAX_DID_YOU_MEAN`] total.
fn did_you_mean(
    requested: &str,
    registered: &[&'static str],
    aliases: &[&'static str],
) -> Vec<&'static str> {
    let lower = requested.to_ascii_lowercase();
    let mut out: Vec<&'static str> = Vec::new();
    for (hint, kind) in DID_YOU_MEAN_HINTS {
        if *hint == lower && !out.contains(kind) {
            out.push(kind);
        }
    }
    let mut scored: Vec<(usize, &'static str)> = registered
        .iter()
        .chain(aliases.iter())
        .filter(|c| !out.contains(*c))
        .map(|c| (levenshtein(&lower, c), *c))
        .filter(|(d, _)| *d <= MAX_DID_YOU_MEAN_DISTANCE)
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
    for (_, candidate) in scored {
        if out.len() >= MAX_DID_YOU_MEAN {
            break;
        }
        out.push(candidate);
    }
    out.truncate(MAX_DID_YOU_MEAN);
    out
}

/// The validator's registration for what this module owns (R3): an unlisted
/// top-level argument (`additionalProperties`) on any control-plane tool is
/// refused by [`check_unknown_argument`] as `unknown_argument`.
pub const ARG_OWNERS: &[crate::host_args::ArgOwner] = &[crate::host_args::ArgOwner {
    applies: |_| true,
    path: "",
    keywords: &["additionalProperties"],
    codes: &["unknown_argument"],
}];

/// PRD-mcphost-spec-unknown-field-rejection requirement 2 (AC4): refuses a
/// top-level `args` key that isn't in `known` with `unknown_argument` --
/// `tenant_key` is always known (requirement 2's own carve-out: every
/// `host_schema`-built tool accepts it as connection auth, never listed in
/// a tool's own hand-written properties). A no-op when `args` isn't a JSON
/// object (the control-plane handler's own argument-shape error takes
/// over). `known` rides straight into `data.known` -- callers pass the
/// tool's own registered property names (AC4: "equal to the registered
/// properties"), never a second hand-maintained list.
pub fn check_unknown_argument(args: &Value, tool: &str, known: &[String]) -> Result<(), AppError> {
    let Some(obj) = args.as_object() else {
        return Ok(());
    };
    for key in obj.keys() {
        if key == "tenant_key" || known.iter().any(|k| k == key) {
            continue;
        }
        return Err(AppError::Structured {
            code: UNKNOWN_ARGUMENT,
            message: format!(
                "'{key}' is not a recognized argument for {tool}; known: {}",
                known.join(", ")
            ),
            data: json!({"field": key, "tool": tool, "known": known}),
        });
    }
    Ok(())
}

/// PRD-mcphost-incident-banner-for-agents R1: the `data` key a host-fault
/// error carries its open-incident pointer under. The one place the name is
/// spelled; `www/llms.txt` and the quickstart error guide are scanned for it
/// by the drift test.
pub const INCIDENT_FIELD: &str = "incident";

/// R1: the error codes that mean "this is the host, not the caller". While an
/// incident is open, exactly these carry `data.<INCIDENT_FIELD>`; a test
/// asserts each is produced as `data.error_code` by a real [`AppError`].
pub const HOST_FAULT_CODES: &[&str] =
    &["storage", "internal", "exec_unavailable", "timeout", "rate_limited_host", "transaction_nested"];

/// PRD-mcphost-named-key-refusal-not-http-401 R1: why a keyless call was
/// refused with `tenant_key_missing`. The HTTP-status decision
/// (`http::upgrade_for`) keys on this, never on whether `data.tenant` exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// No tenant is known for the caller (the original refusal).
    Anonymous,
    /// This connection already named the tenant in the payload.
    ConnectionBound(String),
    /// This peer (IP + User-Agent) redeemed a key in the last 10 minutes.
    /// Carries no namespace: a different agent at the same address must
    /// not learn another tenant's name.
    PeerRecent,
}

impl Reason {
    pub const ALL_WIRE: &'static [&'static str] = &["missing_key", "connection_bound", "peer_recent"];

    /// The `data.reason` string this variant serializes to.
    pub fn wire(&self) -> &'static str {
        match self {
            Reason::Anonymous => "missing_key",
            Reason::ConnectionBound(_) => "connection_bound",
            Reason::PeerRecent => "peer_recent",
        }
    }

    /// Inverse of [`Reason::wire`] for the HTTP middleware, which sees only
    /// the serialized body. Anything unrecognized (including an absent
    /// field) is `Anonymous`, which keeps the 401.
    pub fn from_wire(reason: Option<&str>, tenant: Option<&str>) -> Reason {
        match reason {
            Some("connection_bound") => Reason::ConnectionBound(tenant.unwrap_or_default().to_string()),
            Some("peer_recent") => Reason::PeerRecent,
            _ => Reason::Anonymous,
        }
    }
}

/// `data` key on a named refusal that points at the ways to get a key.
pub const GET_A_KEY: &str = "get_a_key";
pub const GET_A_KEY_TEXT: &str = "call signup for your own key (host.quickstart explains the flow)";
pub const PEER_RECENT_HINT: &str = "a tenant_key was redeemed from your address in the last 10 minutes; if that was you, pass it on every host.* call; if you are a different agent, call signup with handoff:true";

/// The ConnectionBound hint (R3).
pub fn connection_hint(namespace: &str) -> String {
    format!("this connection authenticated as {namespace}; pass its tenant_key on every host.* call")
}

/// Every key a serialized `tenant_key_missing` refusal's `data` may carry.
pub const REFUSAL_FIELDS: &[&str] = &[
    "error_code", "reason", "tenant", "hint", GET_A_KEY, "anchor", "field", "expected", "docs", "example", "request_id",
    "next_call", "class", "family", "feedback_hint", "help_url", "remedy", "retry",
];

#[derive(Debug, Clone, thiserror::Error)]
pub enum AppError {
    #[error("missing or invalid Authorization: Bearer key")]
    Unauthorized,
    /// PRD-mcphost-auth-error-names-argument requirement 1 / AC1: a
    /// `host.*` call with no `Authorization` header and no `tenant_key`
    /// argument (or a non-string one) -- the argument-only auth path
    /// (mcphost-session-key) has no header to be "missing or invalid", so
    /// this names the argument the caller can actually pass instead of
    /// reusing [`AppError::Unauthorized`]'s header-shaped text.
    ///
    /// PRD-mcphost-tenant-key-missing-is-invalid-params requirement 1 / 8:
    /// lives in the `INVALID_PARAMS` jsonrpc arm ([`Self::jsonrpc_code`]),
    /// not `INVALID_REQUEST` alongside [`Self::TenantKeyInvalid`] and the
    /// other auth-failure variants -- this is a missing *argument*, not a
    /// rejected credential, and a client (Claude Code among them) that
    /// branches on the JSON-RPC class rather than `data.error_code` renders
    /// `INVALID_REQUEST` here as "token expired", sending the caller to fix
    /// a token it never had instead of the one string argument it forgot.
    /// The next split of the auth variants should keep it here.
    /// PRD-mcphost-session-bound-tenant-key requirement 2 (AC2, AC3):
    /// `Some(namespace)` when this connection already named a tenant --
    /// either an earlier successful `tenant_key` argument or an implicit
    /// signup this exact session performed (see `AppState::implicit_signup_memory`/
    /// `AppState::tenant_key_arg_memory`) -- so the refusal can name it
    /// (`data.tenant`/`data.hint`) instead of just saying the argument is
    /// missing. `None` keeps the original, unnamed refusal for every other
    /// `host.*`/`billing.*` call with neither header nor argument on a
    /// connection with no memory at all.
    #[error("tenant_key is required: pass the key that signup returned as the tenant_key argument")]
    TenantKeyMissing(Reason),
    /// PRD-mcphost-auth-error-names-argument requirement 2 / AC2-3: a
    /// `tenant_key` argument present but matching no tenant (or a disabled
    /// one, which stays [`AppError::TenantDisabled`] -- unchanged, see
    /// `resolve_tenant_key_auth`). Never interpolates the offending key.
    #[error("tenant_key was not recognized; call signup for a new key or check the value")]
    TenantKeyInvalid,
    #[error("forbidden")]
    Forbidden,
    #[error("tenant is disabled")]
    TenantDisabled { disabled_reason: Option<String>, claim_url: Option<String> },
    #[error("signup rate limit exceeded for this source; try again later")]
    RateLimited,
    #[error("tool not found: {0}")]
    ToolNotFound(String),
    /// PRD-mcphost-tenant-delete AC7: distinct from [`AppError::ToolNotFound`]
    /// so `admin.tenant_delete` on an unknown tenant reads as
    /// `tenant_not_found`, not `tool_not_found`.
    #[error("tenant not found: {0}")]
    TenantNotFound(String),
    /// PRD-mcphost-tenant-delete AC5: `admin.tenant_delete_by_prefix`'s
    /// `prefix` must be at least 4 characters, so a typo (or an omitted
    /// argument) cannot empty the box. Distinct from
    /// [`AppError::InvalidArgs`] because the PRD pins the wire code to
    /// exactly `invalid_params`.
    #[error("invalid params: {0}")]
    InvalidParams(String),
    /// PRD-mcphost-unknown-kind-routes-to-recipe requirement 4 (AC4):
    /// `aliases` (every job-word `control::resolve_kind` already tried
    /// before giving up -- `kinds::aliases::alias_names()`) rides alongside
    /// `registered` so `into_error_data`'s `data.aliases` and
    /// `data.did_you_mean` (an edit-distance/curated-synonym match against
    /// both lists) never drift from the actual alias table.
    #[error("kind '{requested}' is not registered; registered kinds: {registered:?}")]
    UnknownKind {
        requested: String,
        registered: Vec<&'static str>,
        aliases: Vec<&'static str>,
    },
    #[error("invalid tool name '{0}': must match ^[a-z][a-z0-9_]{{1,40}}$")]
    InvalidToolName(String),
    #[error("spec is {0} bytes, over the 64 KiB limit")]
    SpecTooLarge(usize),
    #[error("tenant already holds {0} tools, the maximum")]
    TooManyTools(usize),
    #[error("invalid spec: {0}")]
    InvalidSpec(String),
    /// PRD-mcphost-surface-fluidity requirement 2 (Goal 2): the host used to
    /// carry two wire codes for one mistake -- this variant's own (now
    /// retired) code and the schema pre-check's `args_invalid` below. Both
    /// now emit `args_invalid` (see [`Self::code`]); a client branching on
    /// the error code handles it once. Kept as its own variant (rather than
    /// merged into a single one) because its call sites (missing
    /// control-plane arguments like `host.tool_publish`'s `name`) are
    /// distinct from the generic `tools/call` args-schema pre-check below,
    /// which still builds an [`AppError::Structured`] directly so its extra
    /// `data` fields survive.
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    /// `host.tool_publish` rejected a spec referencing `secret.<name>` for
    /// a secret this tenant hasn't set (AC3).
    #[error("spec references unknown secret '{0}'")]
    SecretMissing(String),
    /// A [`KindError::Structured`] passed through unchanged: a kind-owned
    /// error code (e.g. `host_not_allowed`, `upstream_status`,
    /// `template_error`, `rate_limited`) its own acceptance criteria name
    /// directly. `data` carries extra fields (e.g. `retry_after_s`) merged
    /// into the JSON-RPC error's `data` alongside `error_code`.
    #[error("{message}")]
    Structured {
        code: &'static str,
        message: String,
        data: Value,
    },
    /// Requirement 3 / AC2: two or more simultaneously-invalid fields from
    /// one `Kind::validate_all` call, reported together instead of one per
    /// publish attempt. Built only by [`AppError::from_kind_violations`],
    /// never constructed directly -- `message` is a short summary computed
    /// there ("N fields invalid: a, b"); `errors` holds each violation
    /// already converted to its own `AppError` so [`Self::code`],
    /// [`Self::field_and_expected`] and [`Self::into_error_data`] can reuse
    /// the same per-field logic every other variant already has, keyed off
    /// `errors[0]` for the top-level (back-compat) `error_code`/`field`/
    /// `expected`/`example`, and `errors` in full for the new `data.errors`
    /// array an agent should actually read.
    #[error("{message}")]
    MultiInvalid {
        message: String,
        errors: Vec<AppError>,
    },
    /// PRD-mcphost-call-limits-honest requirement 1: names the deadline
    /// that actually applied to this call -- the declared `timeout_s`
    /// (bounded by the kind's own max) when the spec named one, otherwise
    /// [`crate::state::CALL_TIMEOUT`]'s default -- never a hardcoded "30s"
    /// that may not be what was actually enforced.
    #[error("call timed out after the {0}s deadline")]
    CallTimeout(u64),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error(
        "registry-publish is disabled; set --registry-url / $MCPHOST_REGISTRY_URL to enable it"
    )]
    RegistryDisabled,
    #[error(
        "tenant's domain namespace is not verified; ask the operator to run admin.tenant_verify_namespace"
    )]
    NamespaceUnverified,
    #[error("registry API rejected the publish: {0}")]
    RegistryRejected(String),
    /// PRD-mcphost-sharing requirement 4 (AC6): distinct from the generic
    /// `billing::quota_exceeded` (code `quota_exceeded`) because the PRD
    /// pins this one's wire code to exactly `share_quota_exceeded`, naming
    /// the limit.
    #[error("tenant already shares {used} tools, the plan maximum of {limit}")]
    ShareQuotaExceeded { limit: i64, used: i64 },
    /// PRD-mcphost-sharing requirement 1: `host.group.add`/`host.group.remove`
    /// on a group name this tenant hasn't `host.group.create`d.
    #[error("group not found: {0}")]
    GroupNotFound(String),
    /// PRD-mcphost-handoff-token requirement 2 / AC2: `host.redeem` on a
    /// token matching no row -- never distinguished from "existed once,
    /// long expired and swept" (there is no sweep), same "guessed value
    /// reads identically to a bad one" posture as [`AppError::TenantKeyInvalid`].
    #[error("handoff token was not recognized")]
    HandoffTokenInvalid,
    /// PRD-mcphost-handoff-token requirement 2 / AC2: a second redemption
    /// of an already-claimed token -- distinct code from
    /// [`AppError::HandoffTokenExpired`] so a client (or the AC4
    /// integration test replaying context A's transcript) can tell "this
    /// token worked once already" from "this token's window closed".
    #[error("handoff token has already been redeemed")]
    HandoffTokenRedeemed,
    /// PRD-mcphost-handoff-token requirement 2 / AC2: redemption attempted
    /// after `state::HANDOFF_TOKEN_TTL_SECS` elapsed since issuance.
    #[error("handoff token has expired")]
    HandoffTokenExpired,
    /// PRD-mcphost-tool-versions requirement 4/5 (AC4): `host.tool_rollback`
    /// or a version-pinned call named a version this tool never had (or no
    /// longer has, after retention pruned it) -- names the surviving range
    /// rather than a bare not-found, so a caller can pick a real one without
    /// a round trip through `host.tool_history`.
    #[error("version {requested} not found; valid range is {min}-{max}")]
    VersionNotFound { requested: i64, min: i64, max: i64 },
    /// PRD-mcphost-oauth-resource-server requirement 3: `host.oauth.issuer_set`
    /// on an issuer URL another tenant already registered -- issuers are
    /// globally unique (migration 0038's UNIQUE constraint), never shared
    /// between tenants.
    #[error("issuer already registered by another tenant")]
    IssuerAlreadyRegistered,
    /// requirement 3: a tenant may register at most
    /// [`crate::oauth::MAX_ISSUERS_PER_TENANT`] issuers.
    #[error("tenant already registers {used} issuers, the maximum of {limit}")]
    IssuerQuotaExceeded { limit: i64, used: i64 },
    /// `host.oauth.issuer_remove`/`admin.oauth.jwks_refresh` on an issuer
    /// this tenant never registered (or that no tenant has, for the admin
    /// path).
    #[error("issuer not found: {0}")]
    IssuerNotFound(String),
    /// requirement 5 (AC3, AC5, AC6): bearer JWT validation failed. The
    /// wire `error_code` is always `invalid_token` (RFC 6750's own
    /// bearer-error vocabulary, requirement 5's own wording); the reason
    /// (`unknown_issuer`, `bad_signature`, `expired`, `wrong_audience`, or
    /// `malformed`) is carried separately in `data.error_description` --
    /// see [`Self::into_error_data`].
    #[error("invalid_token: {0}")]
    InvalidToken(&'static str),
    /// requirement 7 (AC8): a `tenant_key` argument and an
    /// `Authorization: Bearer` header both present and resolving to
    /// different tenants.
    #[error(
        "conflicting_credentials: a tenant_key argument and an Authorization bearer resolved to different tenants"
    )]
    ConflictingCredentials,
    /// PRD-mcphost-tenant-resource-metadata requirement 1 (AC2, AC3): a
    /// credential (key or bearer JWT) that resolves to a real tenant, but
    /// not the one named by the `/t/{ns}/mcp` path it was presented on --
    /// never silently served by the wrong tenant.
    #[error("this credential does not belong to the tenant named by this path")]
    WrongTenant,
    /// requirement 4 (AC5): a bearer JWT whose `scope` claim is present but
    /// does not contain `mcp` -- RFC 6750 §3's own wire vocabulary. A token
    /// with no `scope` claim at all is unrestricted (see
    /// `oauth::OauthCaller::scope`'s doc comment).
    #[error("insufficient_scope: the bearer token's scope does not include 'mcp'")]
    InsufficientScope,
    /// PRD-mcphost-url-bound-tenants requirement 1 (AC7): a request on
    /// `/u/{secret}/mcp` that also carries an `Authorization: Bearer` for a
    /// DIFFERENT tenant than the one the path secret named. Distinct from
    /// [`Self::ConflictingCredentials`] (that one's own `tenant_key`-argument
    /// vs. header conflict) because the PRD pins this one's own wire code to
    /// exactly `auth_conflict` -- same INVALID_REQUEST posture, never
    /// silently picking either credential.
    #[error("auth_conflict: this path's URL secret and the Authorization header resolved to different tenants")]
    AuthConflict,
    /// PRD-mcphost-enterprise-managed-auth requirement 1 (AC8): a tenant may
    /// register at most [`crate::oauth::MAX_TRUSTED_ISSUERS_PER_TENANT`]
    /// identity-assertion trusted issuers -- a distinct quota (and wire
    /// code) from [`Self::IssuerQuotaExceeded`]'s bring-your-own-bearer
    /// registry.
    #[error("tenant already trusts {used} issuers, the maximum of {limit}")]
    TrustedIssuerQuotaExceeded { limit: i64, used: i64 },
}

/// One instance of every [`AppError`] variant, plus a `Structured` for each
/// [`crate::feedback::KIND_LEVEL_CODES`] entry: the enumeration the
/// `fbeh_every_code_has_a_kind` drift test walks. Adding a variant without
/// adding it here fails to compile (the exhaustive match in
/// `variant_witness`).
pub fn all_variants_for_test() -> Vec<AppError> {
    fn variant_witness(e: &AppError) {
        match e {
            AppError::Unauthorized
            | AppError::TenantKeyMissing(_)
            | AppError::TenantKeyInvalid
            | AppError::Forbidden
            | AppError::TenantDisabled { .. }
            | AppError::RateLimited
            | AppError::ToolNotFound(_)
            | AppError::TenantNotFound(_)
            | AppError::InvalidParams(_)
            | AppError::UnknownKind { .. }
            | AppError::InvalidToolName(_)
            | AppError::SpecTooLarge(_)
            | AppError::TooManyTools(_)
            | AppError::InvalidSpec(_)
            | AppError::InvalidArgs(_)
            | AppError::SecretMissing(_)
            | AppError::Structured { .. }
            | AppError::MultiInvalid { .. }
            | AppError::CallTimeout(_)
            | AppError::Storage(_)
            | AppError::Internal(_)
            | AppError::RegistryDisabled
            | AppError::NamespaceUnverified
            | AppError::RegistryRejected(_)
            | AppError::ShareQuotaExceeded { .. }
            | AppError::GroupNotFound(_)
            | AppError::HandoffTokenInvalid
            | AppError::HandoffTokenRedeemed
            | AppError::HandoffTokenExpired
            | AppError::VersionNotFound { .. }
            | AppError::IssuerAlreadyRegistered
            | AppError::IssuerQuotaExceeded { .. }
            | AppError::IssuerNotFound(_)
            | AppError::InvalidToken(_)
            | AppError::ConflictingCredentials
            | AppError::WrongTenant
            | AppError::InsufficientScope
            | AppError::AuthConflict
            | AppError::TrustedIssuerQuotaExceeded { .. } => {}
        }
    }
    let s = || "x".to_string();
    let mut all = vec![
        AppError::Unauthorized,
        AppError::TenantKeyMissing(Reason::Anonymous),
        AppError::TenantKeyInvalid,
        AppError::Forbidden,
        AppError::TenantDisabled { disabled_reason: None, claim_url: None },
        AppError::RateLimited,
        AppError::ToolNotFound("t.no_such".into()),
        AppError::TenantNotFound(s()),
        AppError::InvalidParams(s()),
        AppError::UnknownKind { requested: s(), registered: vec!["http"], aliases: vec![] },
        AppError::InvalidToolName(s()),
        AppError::SpecTooLarge(70_000),
        AppError::TooManyTools(100),
        AppError::InvalidSpec(s()),
        AppError::InvalidArgs(s()),
        AppError::SecretMissing(s()),
        AppError::MultiInvalid {
            message: "2 fields invalid".into(),
            errors: vec![AppError::InvalidSpec(s()), AppError::InvalidSpec(s())],
        },
        AppError::CallTimeout(30),
        AppError::Storage(s()),
        AppError::Internal(s()),
        AppError::RegistryDisabled,
        AppError::NamespaceUnverified,
        AppError::RegistryRejected(s()),
        AppError::ShareQuotaExceeded { limit: 1, used: 1 },
        AppError::GroupNotFound(s()),
        AppError::HandoffTokenInvalid,
        AppError::HandoffTokenRedeemed,
        AppError::HandoffTokenExpired,
        AppError::VersionNotFound { requested: 9, min: 1, max: 2 },
        AppError::IssuerAlreadyRegistered,
        AppError::IssuerQuotaExceeded { limit: 1, used: 1 },
        AppError::IssuerNotFound(s()),
        AppError::InvalidToken("expired"),
        AppError::ConflictingCredentials,
        AppError::WrongTenant,
        AppError::InsufficientScope,
        AppError::AuthConflict,
        AppError::TrustedIssuerQuotaExceeded { limit: 1, used: 1 },
    ];
    for code in crate::feedback::KIND_LEVEL_CODES {
        all.push(AppError::Structured { code, message: s(), data: Value::Null });
    }
    all.iter().for_each(variant_witness);
    all
}

/// The header a client echoes to be recognised across calls (`data.session_header`).
pub const SESSION_HEADER: &str = "Mcp-Session-Id";
/// `data.get_a_key.signup`: the explicit way to a key (R2 of
/// PRD-mcphost-implicit-signup-mints-only-on-success).
pub const GET_A_KEY_SIGNUP: &str = "signup";
/// `data.get_a_key.quickstart`: the verb that explains onboarding.
pub const GET_A_KEY_QUICKSTART: &str = "host.quickstart";

impl AppError {
    /// PRD-mcphost-error-responses-invite-feedback R4: a kind-level error
    /// that does not know which tool it ran for (`tool_exception`,
    /// `upstream_status`, `args_invalid`) gets `data.tool` from the call
    /// site that does, so its `feedback_hint` can name it. Never overwrites
    /// a `data.tool` the error already carries.
    pub fn with_tool(self, tool: &str) -> Self {
        match self {
            AppError::Structured { code, message, data }
                if matches!(code, "tool_exception" | "upstream_status" | "args_invalid") =>
            {
                let mut data = match data {
                    Value::Object(m) => m,
                    Value::Null => Map::new(),
                    other => Map::from_iter([("detail".to_string(), other)]),
                };
                data.entry("tool".to_string()).or_insert(json!(tool));
                AppError::Structured { code, message, data: Value::Object(data) }
            }
            other => other,
        }
    }

    /// PRD-mcphost-implicit-signup-mints-only-on-success R2: this error with
    /// `data.get_a_key` -- the two ways to get a key (`signup`,
    /// `host.quickstart`) -- for a keyless first call whose tenant was rolled
    /// back. A non-`Structured` variant is carried as a `Structured` with its own
    /// code, message and data.
    pub fn with_get_a_key(self, base_url: &str) -> Self {
        let get_a_key = json!({
            "signup": GET_A_KEY_SIGNUP,
            "quickstart": GET_A_KEY_QUICKSTART,
        });
        let (code, message, mut data) = match self {
            AppError::Structured { code, message, data } => (code, message, data),
            other => {
                let code = other.code();
                let e = other.into_error_data_at(Some(base_url));
                (code, e.message.to_string(), e.data.unwrap_or(Value::Null))
            }
        };
        let mut obj = match data.take() {
            Value::Object(m) => m,
            Value::Null => Map::new(),
            other => Map::from_iter([("detail".to_string(), other)]),
        };
        obj.insert("get_a_key".to_string(), get_a_key);
        AppError::Structured { code, message, data: Value::Object(obj) }
    }

    /// The machine-readable code every AC-facing test matches on.
    pub fn code(&self) -> &'static str {
        match self {
            // PRD-mcphost-auth-error-names-argument requirement 3 / AC4:
            // renamed from "unauthorized" -- the header path's own code,
            // distinct from the two argument-path codes below now that
            // there are three ways to fail auth instead of one.
            AppError::Unauthorized => BEARER_INVALID,
            AppError::TenantKeyMissing(_) => TENANT_KEY_MISSING,
            AppError::TenantKeyInvalid => TENANT_KEY_INVALID,
            AppError::Forbidden => FORBIDDEN,
            AppError::TenantDisabled { .. } => TENANT_DISABLED,
            AppError::RateLimited => RATE_LIMITED,
            AppError::ToolNotFound(_) => TOOL_NOT_FOUND,
            AppError::TenantNotFound(_) => TENANT_NOT_FOUND,
            AppError::InvalidParams(_) => INVALID_PARAMS,
            AppError::UnknownKind { .. } => UNKNOWN_KIND,
            AppError::InvalidToolName(_) => INVALID_TOOL_NAME,
            AppError::SpecTooLarge(_) => SPEC_TOO_LARGE,
            AppError::TooManyTools(_) => TOO_MANY_TOOLS,
            AppError::InvalidSpec(_) => INVALID_SPEC,
            // PRD-mcphost-surface-fluidity requirement 2: one code for bad
            // arguments -- this used to be a distinct wire code, now it's
            // the same "args_invalid" every other bad-argument rejection in
            // this file already uses.
            AppError::InvalidArgs(_) => ARGS_INVALID,
            AppError::SecretMissing(_) => SECRET_MISSING,
            AppError::Structured { code, .. } => code,
            AppError::MultiInvalid { errors, .. } => {
                errors.first().map(AppError::code).unwrap_or(INVALID_SPEC)
            }
            AppError::CallTimeout(_) => CALL_TIMEOUT,
            AppError::Storage(_) => STORAGE,
            AppError::Internal(_) => INTERNAL,
            AppError::RegistryDisabled => REGISTRY_DISABLED,
            AppError::NamespaceUnverified => NAMESPACE_UNVERIFIED,
            AppError::RegistryRejected(_) => REGISTRY_REJECTED,
            AppError::ShareQuotaExceeded { .. } => SHARE_QUOTA_EXCEEDED,
            AppError::GroupNotFound(_) => GROUP_NOT_FOUND,
            AppError::HandoffTokenInvalid => HANDOFF_TOKEN_INVALID,
            AppError::HandoffTokenRedeemed => HANDOFF_TOKEN_REDEEMED,
            AppError::HandoffTokenExpired => HANDOFF_TOKEN_EXPIRED,
            // AC4: an argument error, same code as every other bad-argument
            // rejection in this file.
            AppError::VersionNotFound { .. } => ARGS_INVALID,
            AppError::IssuerAlreadyRegistered => ISSUER_ALREADY_REGISTERED,
            AppError::IssuerQuotaExceeded { .. } => ISSUER_QUOTA_EXCEEDED,
            AppError::IssuerNotFound(_) => ISSUER_NOT_FOUND,
            AppError::InvalidToken(_) => INVALID_TOKEN,
            AppError::ConflictingCredentials => CONFLICTING_CREDENTIALS,
            AppError::WrongTenant => WRONG_TENANT,
            AppError::InsufficientScope => INSUFFICIENT_SCOPE,
            AppError::AuthConflict => AUTH_CONFLICT,
            AppError::TrustedIssuerQuotaExceeded { .. } => QUOTA_TRUSTED_ISSUERS,
        }
    }
}

crate::err_code! {
    pub ARGS_INVALID => "args_invalid",
    Args, AgentFault, false,
    "Re-send with data.field as data.expected; data.example shows a valid value.",
}

crate::err_code! {
    pub AUTH_CONFLICT => "auth_conflict",
    Auth, AgentFault, false,
    "Send exactly one credential: either the URL secret or the Authorization header, not two different tenants.",
}

crate::err_code! {
    pub BEARER_INVALID => "bearer_invalid",
    Auth, AgentFault, false,
    "Call signup for a key, then send Authorization: Bearer <key> or pass tenant_key as an argument.",
}

crate::err_code! {
    pub CALL_TIMEOUT => "call_timeout",
    Call, AgentFault, true,
    "Reduce the work per call or raise timeout_s within the plan ceiling, then retry.",
}

crate::err_code! {
    pub CONFLICTING_CREDENTIALS => "conflicting_credentials",
    Auth, AgentFault, false,
    "Send one credential: drop the tenant_key argument or the Authorization header.",
}

crate::err_code! {
    pub FORBIDDEN => "forbidden",
    Auth, HostRefusal, false,
    "This credential may not do that; use a key with the needed access.",
}

crate::err_code! {
    pub GROUP_NOT_FOUND => "group_not_found",
    Share, AgentFault, false,
    "List groups and use an id that exists.",
}

crate::err_code! {
    pub HANDOFF_TOKEN_EXPIRED => "handoff_token_expired",
    Auth, AgentFault, false,
    "Request a new handoff token and redeem it before it expires.",
}

crate::err_code! {
    pub HANDOFF_TOKEN_INVALID => "handoff_token_invalid",
    Auth, AgentFault, false,
    "Request a new handoff token; this one is not valid.",
}

crate::err_code! {
    pub HANDOFF_TOKEN_REDEEMED => "handoff_token_redeemed",
    Auth, AgentFault, false,
    "Request a new handoff token; each token redeems once.",
}

crate::err_code! {
    pub INSUFFICIENT_SCOPE => "insufficient_scope",
    Auth, HostRefusal, false,
    "Use a token that carries the scope named in data.scope.",
}

crate::err_code! {
    pub INTERNAL => "internal",
    Internal, HostFault, true,
    "Retry once; if it persists, report data.request_id through the support link.",
}

crate::err_code! {
    pub INVALID_PARAMS => "invalid_params",
    Args, AgentFault, false,
    "Re-send with the params the method expects; data names the limit or field.",
}

crate::err_code! {
    pub INVALID_SPEC => "invalid_spec",
    Spec, AgentFault, false,
    "Fix the spec field named in data.field to match data.expected.",
    data_keys: crate::kinds::kinddoc::INVALID_SPEC_DATA_KEYS,
}

crate::err_code! {
    pub INVALID_TOKEN => "invalid_token",
    Auth, AgentFault, false,
    "Fetch a new token; data.error_description says whether it was expired, malformed or from an unknown issuer.",
}

crate::err_code! {
    pub INVALID_TOOL_NAME => "invalid_tool_name",
    Args, AgentFault, false,
    "Use a tool name of lowercase letters, digits and underscores within the length limit.",
}

crate::err_code! {
    pub ISSUER_ALREADY_REGISTERED => "issuer_already_registered",
    Oauth, AgentFault, false,
    "Use the existing issuer registration or remove it first.",
}

crate::err_code! {
    pub ISSUER_NOT_FOUND => "issuer_not_found",
    Oauth, AgentFault, false,
    "List issuers and use a registered one.",
}

crate::err_code! {
    pub ISSUER_QUOTA_EXCEEDED => "issuer_quota_exceeded",
    Plan, PlanLimit, false,
    "Remove an unused issuer; data.limit is the plan's maximum.",
}

crate::err_code! {
    pub NAMESPACE_UNVERIFIED => "namespace_unverified",
    Registry, HostRefusal, false,
    "Verify the namespace with the registry before publishing.",
}

crate::err_code! {
    pub QUOTA_TRUSTED_ISSUERS => "quota_trusted_issuers",
    Plan, PlanLimit, false,
    "Remove a trusted issuer; data.limit is the plan's maximum.",
}

crate::err_code! {
    pub RATE_LIMITED => "rate_limited",
    Plan, PlanLimit, true,
    "Wait data.retry_after_s seconds and retry.",
}

crate::err_code! {
    pub REGISTRY_DISABLED => "registry_disabled",
    Registry, HostRefusal, false,
    "This host has no registry enabled; do not retry.",
}

crate::err_code! {
    pub REGISTRY_REJECTED => "registry_rejected",
    Registry, UpstreamFault, false,
    "The registry refused the publish; correct the spec and retry.",
}

crate::err_code! {
    pub SECRET_MISSING => "secret_missing",
    Call, AgentFault, false,
    "Set the missing secret with host.secret_set, then retry.",
}

crate::err_code! {
    pub SHARE_QUOTA_EXCEEDED => "share_quota_exceeded",
    Plan, PlanLimit, false,
    "Unshare a tool or move to a plan with a higher share limit.",
}

crate::err_code! {
    pub SPEC_TOO_LARGE => "spec_too_large",
    Plan, PlanLimit, false,
    "Reduce the spec below the size cap, for example by moving data to host.blob.put.",
}

crate::err_code! {
    pub STORAGE => "storage",
    Storage, HostFault, true,
    "Retry once; if it persists, report data.request_id through the support link.",
}

crate::err_code! {
    pub TENANT_DISABLED => "tenant_disabled",
    Auth, HostRefusal, false,
    "Read data.disabled_reason. self_offboard: this account was closed by self_offboard; call any host.* with no key on a new session (or signup) to start fresh. admin_disable: do not retry.",
}

crate::err_code! {
    pub TENANT_KEY_INVALID => "tenant_key_invalid",
    Auth, AgentFault, false,
    "Call signup for a fresh key; this tenant_key matched no tenant.",
}

crate::err_code! {
    pub TENANT_KEY_MISSING => "tenant_key_missing",
    Auth, AgentFault, false,
    "Pass tenant_key, or reconnect through your onboarding url; data.tenant names the tenant when known.",
}

crate::err_code! {
    pub TENANT_NOT_FOUND => "tenant_not_found",
    Auth, AgentFault, false,
    "Check the namespace spelling and use a tenant that exists.",
}

crate::err_code! {
    pub TOO_MANY_TOOLS => "too_many_tools",
    Plan, PlanLimit, false,
    "Remove a tool or move to a plan with a higher tool limit.",
}

crate::err_code! {
    pub TOOL_NOT_FOUND => "tool_not_found",
    Call, AgentFault, false,
    "Check the name against host.tool_list; shared tools are called as <owner_namespace>.<tool>.",
}

crate::err_code! {
    pub UNKNOWN_KIND => "unknown_kind",
    Spec, AgentFault, false,
    "Choose a kind from data.registered; data.did_you_mean suggests a near match.",
}

crate::err_code! {
    pub WRONG_TENANT => "wrong_tenant",
    Auth, AgentFault, false,
    "Use the credential of the tenant this URL belongs to.",
}

impl AppError {
    fn jsonrpc_code(&self) -> ErrorCode {
        match self {
            AppError::ToolNotFound(_) | AppError::TenantNotFound(_) | AppError::GroupNotFound(_) => {
                ErrorCode::RESOURCE_NOT_FOUND
            }
            AppError::UnknownKind { .. }
            | AppError::InvalidToolName(_)
            | AppError::SpecTooLarge(_)
            | AppError::TooManyTools(_)
            | AppError::InvalidSpec(_)
            | AppError::InvalidArgs(_)
            | AppError::InvalidParams(_)
            | AppError::ShareQuotaExceeded { .. }
            | AppError::VersionNotFound { .. }
            | AppError::IssuerAlreadyRegistered
            | AppError::IssuerQuotaExceeded { .. }
            | AppError::TrustedIssuerQuotaExceeded { .. }
            | AppError::SecretMissing(_)
            // PRD-mcphost-tenant-key-missing-is-invalid-params requirement 1
            // (AC1-5): moved out of the INVALID_REQUEST arm below -- a
            // missing/non-string tenant_key argument is a bad parameter,
            // the same class as every other "you forgot an argument" error
            // here, not a rejected credential.
            | AppError::TenantKeyMissing(_) => ErrorCode::INVALID_PARAMS,
            AppError::IssuerNotFound(_) => ErrorCode::RESOURCE_NOT_FOUND,
            AppError::Storage(_)
            | AppError::Internal(_)
            | AppError::CallTimeout(_)
            | AppError::RegistryRejected(_) => ErrorCode::INTERNAL_ERROR,
            AppError::Unauthorized
            | AppError::TenantKeyInvalid
            | AppError::Forbidden
            | AppError::TenantDisabled { .. }
            | AppError::RateLimited
            | AppError::RegistryDisabled
            | AppError::NamespaceUnverified
            | AppError::HandoffTokenInvalid
            | AppError::HandoffTokenRedeemed
            | AppError::HandoffTokenExpired
            | AppError::InvalidToken(_)
            | AppError::ConflictingCredentials
            | AppError::WrongTenant
            | AppError::InsufficientScope
            | AppError::AuthConflict => ErrorCode::INVALID_REQUEST,
            // `host_not_allowed`/`args_invalid`/`template_error` are caller
            // (or spec-author) input problems; `rate_limited` mirrors
            // AppError::RateLimited above; the remaining `upstream_*` /
            // `response_too_large` codes are upstream-side failures the
            // host itself didn't cause, so they line up with the other
            // INTERNAL_ERROR-mapped variants above.
            AppError::Structured { code, .. } => match *code {
                // PRD-mcphost-signup-kill-switch-and-source requirement 3:
                // a paused `signup` is the same "come back later" caller-
                // state gate as `rate_limited` right above it, not a bad
                // argument and not an internal failure.
                // PRD-mcphost-abuse-guard-ban-list requirement 2: a banned
                // subject is the same "come back later/never" caller-state
                // gate as a paused signup or a rate limit, not a bad
                // argument and not an internal failure.
                // PRD-mcphost-tool-scopes-and-consent requirement 4 (AC2):
                // a per-tool `insufficient_scope` refusal is the same
                // caller-credential problem `AppError::InsufficientScope`
                // already maps here.
                // PRD-mcphost-invite-links requirement 4 (AC5) / requirement
                // 3/11 (AC6, AC10): the same caller-state-gate class as
                // `rate_limited`/`banned` above -- never a bad argument,
                // never an internal failure.
                // PRD-mcphost-implicit-signup requirement 3: the implicit-
                // signup path's own rate-limit refusal is the same
                // caller-state gate as `rate_limited` right above it.
                "rate_limited" | "signup_rate_limited" | "signup_paused" | "banned"
                // PRD-mcphost-key-scoped-to-tools R3: a sub-key calling
                // outside its allowlist is the same caller-credential
                // problem as `insufficient_scope`.
                | "permission_denied"
                | crate::scopes::INSUFFICIENT_SCOPE | "invite_invalid" | "invite_rate_limited" | "join_code_invalid" => {
                    ErrorCode::INVALID_REQUEST
                }
                // PRD-mcphost-invite-links requirement 1 (AC7): the same
                // caller-input-quota class as `state_quota_exceeded` above.
                // PRD-mcphost-paged-trait-on-every-list-verb requirement 2: a
                // cursor this process did not mint is a bad parameter.
                "invites_quota_exceeded" | "org_members_max" | "resources_max" | "listen_streams_max" | "org_member_already" | "org_name_taken"
                | "not_in_org" | "last_client" | "client_not_found" | "cursor_invalid" | "field_unknown" | "idempotency_conflict" => {
                    ErrorCode::INVALID_PARAMS
                }
                // PRD-mcphost-destructive-verb-confirmation-token R1/R3: a
                // large destructive call without a (valid) `confirm` token
                // is a caller-input problem the error itself explains.
                "confirm_required" | "confirm_invalid" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-spec-output-paths requirement 1: a structured
                // `invalid_spec` (kinds::http/python's own `parse_spec` and
                // `normalize_outputs`) is exactly the same caller-input
                // problem `AppError::InvalidSpec` already maps to
                // INVALID_PARAMS above -- it must not fall into this match's
                // `_ => INTERNAL_ERROR` default just because it arrives via
                // `KindError::Structured` instead of `KindError::InvalidSpec`.
                "host_not_allowed" | "args_invalid" | "template_error" | "kind_mismatch"
                | "jq_builtin_unsupported"
                | "invalid_spec"
                // PRD-mcphost-spec-unknown-field-rejection requirement 1/2: a
                // spec or control-plane call carrying a key the server
                // doesn't understand is the same caller-input problem as
                // `invalid_spec`/`args_invalid` above.
                | "unknown_spec_field" | "unknown_argument"
                // PRD-mcphost-event-trigger-self-test P0 requirement 5 (AC8):
                // host.quickstart's own hinted rejection for a trigger-kind
                // word (schedule/event/webhook) is the same caller-input
                // problem as AppError::UnknownKind's own INVALID_PARAMS
                // mapping -- just carried via Structured so it can add
                // `data.hint`.
                | "unknown_kind"
                // PRD-mcphost-python-kind-plain-env requirement 3 (AC4): a
                // publish/secret_set naming a colliding env/secret key is
                // the same caller-input problem as `invalid_spec` above.
                | "env_secret_collision" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-tenant-state requirement 1/4: a schema
                // violation, a quota overrun, and an undeclared table are
                // all caller-input problems (a bad `host.state.insert`
                // payload, a write past a plan's quota, a typo'd table
                // name) -- the same INVALID_PARAMS bucket as `invalid_spec`
                // above, not an internal failure.
                "state_schema_violation" | "state_quota_exceeded" | "state_table_not_found" => {
                    ErrorCode::INVALID_PARAMS
                }
                // PRD-mcphost-first-call-gift requirement 1 (AC2): an
                // over-long `remember` argument is the same caller-input
                // problem as the `state_*` group above.
                "remember_too_long" | "invalid_params" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-oauth-metadata-lists-hosted-as R2: a malformed
                // issuer on `issuer_set`/`trusted_issuer_set`/`provider_set`
                // is the same caller-input problem.
                "issuer_url_invalid" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-schedules requirements 2/4: a bad cron
                // expression, an over-quota schedules_max, or a too-short
                // schedule_min_interval_s are all caller-input problems,
                // same INVALID_PARAMS bucket as the `state_*` group above;
                // `trigger_not_found` mirrors `tool_not_found`/
                // `tenant_not_found`'s own RESOURCE_NOT_FOUND.
                // PRD-mcphost-webhook-inbox-one-call R6: `plan_limit` is the
                // same caller-input refusal for `host.webhook_inbox.create`.
                "trigger_invalid" | "trigger_quota_exceeded" | "trigger_interval_too_short" | "plan_limit" => {
                    ErrorCode::INVALID_PARAMS
                }
                // PRD-mcphost-ask-user-decision: a lapsed ask is a caller-state
                // problem (create a new one); an unknown ask id is a not-found.
                "ask_expired" | "asks_limit" => ErrorCode::INVALID_PARAMS,
                "ask_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-notify-user-channel: caller-input and caller-state refusals.
                "notify_kind_unknown" | "owner_unverified" | "email_not_owner" | "no_channel" | "notify_limit"
                | "notify_channel_limit" | "notify_target_invalid" | "email_quota_exceeded" | "email_owner_unverified" => {
                    ErrorCode::INVALID_PARAMS
                }
                "notify_channel_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-send-email-from-tool: `email_provider_error` is a host-side failure
                // and stays on the default mapping.
                "trigger_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-call-trace-replay: an unknown/expired trace
                // mirrors `tool_not_found`; a replay left holding a masked
                // value is a caller-input problem.
                "trace_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-key-scoped-to-tools R2: `host.key.mint` naming a
                // tool that is not the caller's own or shared in.
                "not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-key-scoped-to-tools R3: a revoked or expired
                // sub-key is a bad credential parameter (-32602).
                "key_expired" | "key_revoked" | "key_bound_elsewhere" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-call-audit-export R3: an export window over the
                // row cap is a caller-input problem (narrow the window).
                "too_many_rows" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-key-delegate-child-keys R2/R4: a delegation
                // asking for more than its parent, or a revoke that would
                // orphan live children, is a caller-input problem.
                crate::scopes::SCOPE_ESCALATION | "has_children" => ErrorCode::INVALID_PARAMS,
                "replay_needs_secret" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-inbound-events: `host.trigger.test`'s own
                // caller-input problems -- a bad signature/payload, a rate
                // ceiling, an oversized body -- same INVALID_PARAMS/
                // INVALID_REQUEST split the `state_*`/`trigger_*` groups
                // above already use; `hook_not_found` mirrors
                // `trigger_not_found`'s RESOURCE_NOT_FOUND (only reachable
                // here via `host.trigger.test` on an unknown/wrong-kind
                // trigger -- `POST /hooks/...` itself never goes through
                // this jsonrpc mapping, it builds its own HTTP status).
                "signature_invalid" | "event_body_too_large" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-alerting-webhook requirement 5 / AC11: an
                // oversized `admin.alerts.raise` body is the same
                // caller-input problem as `event_body_too_large` above.
                "alert_body_too_large" => ErrorCode::INVALID_PARAMS,
                "events_rate_limited" => ErrorCode::INVALID_REQUEST,
                "hook_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-tenant-tables requirements 1-6: a schema
                // violation, an undeclared table, or a structurally
                // rejected (non-SELECT, multi-statement, or over a row/time
                // bound) `host.table.query` are all caller-input problems,
                // the same INVALID_PARAMS bucket the `state_*` group above
                // already uses for the analogous KV-store rejections.
                "table_schema_violation" | "table_not_found" | "table_query_rejected"
                | "table_bound_exceeded" | "table_already_exists" => ErrorCode::INVALID_PARAMS,
                c if c == crate::tables::TABLE_ROW_CONFLICT => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-document-store P0 requirements 2-5: a too-large
                // put, an unsupported mime, or either quota overrun are all
                // caller-input problems, same INVALID_PARAMS bucket as the
                // `table_*`/`state_*` groups above; `docs_not_found` mirrors
                // `table_not_found`'s own RESOURCE_NOT_FOUND.
                "docs_too_large" | "docs_mime_unsupported" | "quota_docs" | "quota_docs_bytes" => {
                    ErrorCode::INVALID_PARAMS
                }
                "docs_not_found" | "blob_not_found" | "blob_expired" | "index_not_found" => {
                    ErrorCode::RESOURCE_NOT_FOUND
                }
                // PRD-mcphost-index-create-from-sources: a duplicate name or
                // the `index_max` ceiling is a caller-input problem, same
                // INVALID_PARAMS bucket as `quota_docs`.
                "index_exists" | "quota_index_max" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-blob-put-get-signed-url R3/R4: the blob quota
                // and size refusals are the same caller-input class as the
                // `quota_docs` group above.
                "quota_blobs" | "quota_blob_bytes" | "blob_too_large" | "blob_too_large_inline" => {
                    ErrorCode::INVALID_PARAMS
                }
                // PRD-mcphost-agent-directory requirement 3 (AC3/AC9): a
                // handle collision or a claim against the reserved list is
                // the same caller-input problem as the `table_*`/`state_*`
                // groups above.
                "handle_taken" | "handle_reserved" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-shared-tool-spec-readback requirement 4: a
                // tool shared with the caller but not `expose_spec: true`
                // is a caller-input problem naming the tool, same
                // INVALID_PARAMS bucket as `handle_taken`/`handle_reserved`
                // above (never `tool_not_found`'s RESOURCE_NOT_FOUND -- the
                // caller already knows the tool exists, since it was shared
                // with them).
                "spec_not_exposed" => ErrorCode::INVALID_PARAMS,
                // Requirement 4 (AC4): `agent_not_found` mirrors
                // `tool_not_found`/`tenant_not_found`/`trigger_not_found`'s
                // own RESOURCE_NOT_FOUND. PRD-mcphost-agent-inbox
                // requirement 3 (AC3): `thread_not_found` is the same
                // shape, one more caller-visible "doesn't exist" id.
                // PRD-mcphost-agent-consent requirement 3: same "doesn't
                // exist" shape as the two above.
                // PRD-mcphost-agent-mesh-ops requirement 4/AC4: `channel_not_found`
                // mirrors the other "doesn't exist" codes above; `mesh_frozen`
                // is a caller-state gate, same INVALID_REQUEST bucket as
                // `tenant_disabled`/`rate_limited` above (not a bad argument,
                // not an internal failure).
                "agent_not_found" | "thread_not_found" | "contact_request_not_found"
                | "channel_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-agent-channels requirement 2 / P1 requirement
                // 10: a closed/frozen channel is a caller-state gate, same
                // INVALID_REQUEST bucket as `mesh_frozen` above.
                "mesh_frozen" | "channel_closed" | "channel_frozen" => ErrorCode::INVALID_REQUEST,
                // PRD-mcphost-end-user-identity: `end_user_required`/
                // `end_user_explicit_forbidden`/`quota_end_users` are
                // caller-input problems, same INVALID_PARAMS bucket as the
                // `state_*` group above; `end_user_assertion_invalid` is an
                // auth-shaped rejection, same INVALID_REQUEST bucket as
                // `invalid_token` above.
                "end_user_required" | "end_user_explicit_forbidden" | "quota_end_users" => {
                    ErrorCode::INVALID_PARAMS
                }
                "end_user_assertion_invalid" => ErrorCode::INVALID_REQUEST,
                // PRD-mcphost-shared-tool-call-path requirement 2:
                // `AppError::shared_tool_not_found`'s own code, mirroring
                // `AppError::ToolNotFound`'s RESOURCE_NOT_FOUND above -- a
                // `Structured` variant only because it carries `data.hint`,
                // not because it's a different class of error.
                "tool_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-tool-call-host-verb-forward requirement 3
                // (AC5): `host.tool_test` on a forwardable host verb with no
                // dry run yet -- a caller-input problem (the caller asked
                // for a verification mode this verb doesn't support), same
                // bucket as `end_user_assertion_invalid` above, never
                // `tool_not_found`'s RESOURCE_NOT_FOUND (the verb itself was
                // found and is real; only its dry run is missing).
                "unverifiable" => ErrorCode::INVALID_REQUEST,
                // PRD-mcphost-lineage-blast-radius requirement 7: a drop
                // refused because a breaking consumer exists is a
                // caller-input problem (the caller omitted `confirm: true`),
                // same INVALID_PARAMS bucket as `table_not_found` above.
                "lineage_blocked" => ErrorCode::INVALID_PARAMS,
                "lineage_handle_not_found" => ErrorCode::RESOURCE_NOT_FOUND,
                // PRD-mcphost-row-policy requirement 7: `host.policy.*`/
                // `host.audit.*` refusing an end-user credential is the
                // same caller-credential-authority gate as
                // `insufficient_scope` above, not a bad argument;
                // `policy_widening` (`host.policy.set` refusing a rule
                // change without `replace: true`) is a caller-input
                // problem, same INVALID_PARAMS bucket as `invalid_spec`.
                "tenant_key_required" => ErrorCode::INVALID_REQUEST,
                "policy_widening" => ErrorCode::INVALID_PARAMS,
                // PRD-mcphost-run-budget-governor requirement 3 (AC3): a
                // per-call `budget` override above the plan's own ceiling is
                // a caller-input problem, same INVALID_PARAMS bucket as the
                // `state_*`/`table_*` groups above; requirement 4: a run
                // whose ledger crossed `Exceeded` is a caller-state gate
                // (the run itself, not this one call, is at fault), same
                // INVALID_REQUEST bucket as `rate_limited`/`banned` above.
                "budget_ceiling_exceeded" => ErrorCode::INVALID_PARAMS,
                "budget_exceeded" => ErrorCode::INVALID_REQUEST,
                _ => ErrorCode::INTERNAL_ERROR,
            },
            AppError::MultiInvalid { errors, .. } => errors
                .first()
                .map(AppError::jsonrpc_code)
                .unwrap_or(ErrorCode::INVALID_PARAMS),
        }
    }

    /// Splits a "<field>: <rest>" convention message -- the pattern every
    /// `KindError::InvalidSpec`/`InvalidArgs`/`Structured` message in this
    /// crate already follows (`method: must be one of ...`, `source: must
    /// not be empty`, `url: rendered host '...' is not publicly callable`)
    /// -- into the field name and the human-readable "expected" tail.
    /// Guards against splitting an ordinary sentence that happens to
    /// contain ": " (e.g. "near line 3: unexpected token") by requiring the
    /// candidate field to look like an identifier/JSON-pointer segment, not
    /// prose (no spaces).
    fn split_field(message: &str) -> Option<(&str, &str)> {
        let (field, rest) = message.split_once(": ")?;
        if field.is_empty() || field.contains(' ') {
            return None;
        }
        Some((field, rest.trim()))
    }

    /// The `field`/`expected` pair for every rejection this crate can
    /// return (requirement 2 / AC2, AC5): named `AppError` variants get a
    /// hand-written field and expectation naming the exact control-plane
    /// argument at fault; `InvalidSpec`/`InvalidArgs` and any `Structured`
    /// kind error fall back to the shared "<field>: <rest>"
    /// message convention via [`Self::split_field`] -- so a kind gains this
    /// for free by writing its error messages the way every kind already
    /// does, no enum change or call-site rewrite required.
    fn field_and_expected(&self) -> (Option<String>, Option<String>) {
        match self {
            // PRD-mcphost-tenant-key-missing-is-invalid-params requirement
            // 4 (AC6): names the argument and both ways to satisfy it, so a
            // caller (or its agent) reading `data` can fix its next call
            // without parsing the message.
            AppError::TenantKeyMissing(_) => (
                Some("tenant_key".to_string()),
                Some(
                    "the string signup returned; required only when this connection \
                     carries no Authorization: Bearer header"
                        .to_string(),
                ),
            ),
            AppError::InvalidToolName(name) => (
                Some("name".to_string()),
                Some(format!("must match ^[a-z][a-z0-9_]{{1,40}}$; got '{name}'")),
            ),
            AppError::UnknownKind { registered, .. } => (
                Some("kind".to_string()),
                Some(format!("must be one of {registered:?}")),
            ),
            AppError::SpecTooLarge(n) => (
                Some("spec".to_string()),
                Some(format!(
                    "serializes to {n} bytes; must be at most {} bytes",
                    crate::state::MAX_SPEC_BYTES
                )),
            ),
            AppError::SecretMissing(name) => (
                Some("spec".to_string()),
                Some(format!(
                    "references secret.{name}, which this tenant has not set; \
                     call host.secret_set first"
                )),
            ),
            AppError::InvalidArgs(m) | AppError::InvalidSpec(m) => {
                match Self::split_field(m) {
                    Some((field, expected)) => {
                        (Some(field.to_string()), Some(expected.to_string()))
                    }
                    None => (None, Some(m.clone())),
                }
            }
            AppError::Structured { message, data, .. } => {
                let field_from_data = data
                    .get("field")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                // PRD-mcphost-spec-output-paths requirement 1: a structured
                // `invalid_spec` already carries its own clean `expected`
                // phrase in `data` (distinct from `got`/`example`) -- prefer
                // it over the generic "<field>: <rest>" message-split below,
                // which would otherwise clobber it with the *whole* message
                // (this variant's own `message` deliberately embeds an
                // `"invalid spec: "` prefix per requirement 1's wire format,
                // which defeats `split_field`'s "no spaces in the field"
                // guard and falls through to the whole-message fallback).
                // No existing `Structured` error sets `data.expected`, so
                // this is purely additive for every caller that predates
                // this PRD.
                let expected_from_data = data
                    .get("expected")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let split = Self::split_field(message);
                let field = field_from_data.or_else(|| split.map(|(f, _)| f.to_string()));
                let expected = expected_from_data
                    .or_else(|| split.map(|(_, e)| e.to_string()))
                    .or_else(|| Some(message.clone()));
                (field, expected)
            }
            // Back-compat top-level field/expected (requirement 2): the
            // first violation's own, same as if it had been the only one.
            // Every violation (including this first one again) also gets
            // its own entry in `into_error_data`'s `errors` array below.
            AppError::MultiInvalid { errors, .. } => errors
                .first()
                .map(AppError::field_and_expected)
                .unwrap_or((None, None)),
            _ => (None, None),
        }
    }

    /// Zero-arg convenience for the many call sites with no request-scoped
    /// base URL in hand (hermetic unit tests, `kinds::conformance`'s shape
    /// checks, the free function [`crate::handler::get_parts`]'s own
    /// no-HTTP-transport edge case) -- same wire shape as
    /// [`Self::into_error_data_at`] with `base_url: None`, which just means
    /// no `help_url` is attached (PRD-mcphost-first-hour-support-surface
    /// requirement 1 names `help_url` as present "for every code in a
    /// table `HELP_CODES`", not unconditionally -- a response built with no
    /// base url to build one from is the one case it's legitimately
    /// absent).
    pub fn into_error_data(self) -> ErrorData {
        self.into_error_data_at(None)
    }

    /// `base_url`: this host's own externally-reachable origin for *this*
    /// request (PRD-mcphost-first-hour-support-surface technical
    /// considerations: "the request's Host header or `MCPHOST_PUBLIC_URL`")
    /// -- every real HTTP call site in `handler.rs` passes
    /// `Some(&self.state.public_url)`, the same base every other generated
    /// link on this host (`claim::claim_url`, `authz`'s issuer/resource
    /// URIs, `webhooks`/`hooks`' callback URLs) already uses, so `help_url`
    /// never disagrees with where those links point. `None` only when no
    /// such base exists at all (see [`Self::into_error_data`]'s doc
    /// comment).
    pub fn into_error_data_at(self, base_url: Option<&str>) -> ErrorData {
        self.into_error_data_attempt(base_url, 1)
    }

    /// [`Self::into_error_data_at`] for a caller-reported `_meta.attempt`
    /// (PRD-mcphost-retry-policy-table-generated R2): `data.retry`'s
    /// `attempts_left` and `after_ms` are computed for that attempt.
    pub fn into_error_data_attempt(self, base_url: Option<&str>, attempt: u32) -> ErrorData {
        let code = self.code();
        let jsonrpc_code = self.jsonrpc_code();
        let request_id = generate_request_id();
        // Audit finding 2 (mcphost-polish-p0-20260930), generalized by
        // requirement 1/6 of PRD-mcphost-first-hour-support-surface: every
        // other variant's `Display` is operator-authored, safe-by-
        // construction copy -- these four codes alone can carry raw
        // upstream/host detail (`Internal`: Stripe bodies, `io::Error`/
        // pkcs8 decode text; `Storage`: a raw `rusqlite::Error`'s text;
        // `RegistryRejected`: the registry API's own rejection body;
        // `service_unavailable` (`Self::disk_floor`): free/floor byte
        // counts), so these alone get swapped for a generic message plus a
        // correlation id -- the detail still reaches an operator, just via
        // the server log instead of the wire. Every other 5xx-mapped
        // `Structured` code (`sandbox_unavailable`, `dependency_advisory`,
        // `plan_required`, ...) is deliberately excluded: those messages
        // are operator-authored and already pinned verbatim by their own
        // PRDs' acceptance criteria (e.g.
        // `tests/mcphost_python_dependency_policy_ac05_near_name_denied.rs`),
        // not a leak this PRD's audit found -- rewriting them would be
        // scope creep that breaks passing tests, not a fix.
        let message = match code {
            "internal" | "storage" | "registry_rejected" | "service_unavailable" => {
                let detail = self.to_string();
                tracing::error!(request_id = %request_id, code = %code, detail = %detail, "service error");
                if code == "service_unavailable" {
                    format!("service unavailable; request_id={request_id}")
                } else {
                    format!("internal error; request_id={request_id}")
                }
            }
            _ => self.to_string(),
        };
        let (field, expected) = self.field_and_expected();
        let example = field.as_deref().and_then(field_example);
        let mut obj: Map<String, Value> = match &self {
            AppError::Structured { data, .. } if data.is_object() => {
                data.as_object().cloned().unwrap_or_default()
            }
            // R2: a non-object `data` is kept as `detail` beside the hint.
            AppError::Structured { data, .. } if !data.is_null() => {
                let mut m = Map::new();
                m.insert("detail".to_string(), data.clone());
                m
            }
            _ => Map::new(),
        };
        obj.insert("error_code".to_string(), json!(code));
        // requirement 5 (AC3, AC5, AC6): the reason `error_code` alone
        // can't carry (every bearer-validation failure shares the same
        // `invalid_token` code) -- `unknown_issuer`, `bad_signature`,
        // `expired`, `wrong_audience`, or `malformed`.
        if let AppError::InvalidToken(reason) = &self {
            obj.insert("error_description".to_string(), json!(reason));
        }
        if let AppError::IssuerQuotaExceeded { limit, used } = &self {
            obj.insert("limit".to_string(), json!(limit));
            obj.insert("used".to_string(), json!(used));
        }
        if let AppError::TrustedIssuerQuotaExceeded { limit, used } = &self {
            obj.insert("limit".to_string(), json!(limit));
            obj.insert("used".to_string(), json!(used));
        }
        // PRD-mcphost-session-bound-tenant-key requirement 2 (AC2, AC3):
        // `data.tenant` (namespace only, never the key) and a hint naming
        // it, so a caller that dropped `tenant_key` learns which tenant
        // this connection already is without parsing the message.
        // PRD-mcphost-error-envelope-schema-contract AC6: the machine-readable
        // reason a tenant-key rejection carries, so a client branches on
        // `data.reason` rather than the message, on every verb alike.
        if matches!(self, AppError::TenantKeyInvalid) {
            obj.insert("reason".to_string(), json!("unknown_key"));
        }
        if let AppError::TenantKeyMissing(reason) = &self {
            obj.insert("reason".to_string(), json!(reason.wire()));
            match reason {
                Reason::Anonymous => {}
                Reason::ConnectionBound(namespace) => {
                    obj.insert("tenant".to_string(), json!(namespace));
                    obj.insert("hint".to_string(), json!(connection_hint(namespace)));
                    obj.insert(GET_A_KEY.to_string(), json!(GET_A_KEY_TEXT));
                }
                Reason::PeerRecent => {
                    obj.insert("hint".to_string(), json!(PEER_RECENT_HINT));
                    obj.insert("session_header".to_string(), json!(SESSION_HEADER));
                    obj.insert(GET_A_KEY.to_string(), json!(GET_A_KEY_TEXT));
                }
            }
        }
        // R6 (PRD-mcphost-self-offboard-success-envelope): a disabled tenant
        // error carries its reason so callers can distinguish self_offboard
        // from admin_disable.
        if let AppError::TenantDisabled { disabled_reason: Some(reason), claim_url } = &self {
            obj.insert("disabled_reason".to_string(), json!(reason));
            // PRD-mcphost-external-unclaimed-tenant-expiry: the claim link of
            // a tenant invalidated as unclaimed, for its credential's holder.
            if let Some(claim_url) = claim_url {
                obj.insert("claim_url".to_string(), json!(claim_url));
            }
        }
        // PRD-mcphost-unknown-kind-routes-to-recipe requirement 4 (AC4):
        // `registered`/`aliases` ride straight through from the variant;
        // `did_you_mean` is derived from both, never hand-maintained.
        if let AppError::UnknownKind { requested, registered, aliases } = &self {
            obj.insert("registered".to_string(), json!(registered));
            obj.insert("aliases".to_string(), json!(aliases));
            // PRD-mcphost-kind-ask-routing requirement 2 (AC1): an outcome
            // word ("docs", "message", ...) answers with the verbs/example
            // that deliver it; any other unknown kind keeps the
            // edit-distance list.
            let suggestion = match crate::kinds::outcomes::find(requested) {
                Some(outcome) => outcome.to_json(),
                None => json!(did_you_mean(requested, registered, aliases)),
            };
            obj.insert("did_you_mean".to_string(), suggestion);
        }
        if let Some(field) = field {
            obj.insert("field".to_string(), json!(field));
        }
        // `entry`, not `insert`: a structured error whose own `data.expected`
        // is not a string (`fixture_shape_mismatch`'s key list) keeps it.
        if let Some(expected) = expected {
            obj.entry("expected".to_string()).or_insert(json!(expected));
        }
        if let Some(example) = example {
            obj.insert("example".to_string(), example);
        }
        // Requirement 3 / AC2: every simultaneously-failing field, not just
        // the first -- one entry per violation, each with its own
        // `field`/`expected`/`example` built the same way the top-level
        // ones above are.
        if let AppError::MultiInvalid { errors, .. } = &self {
            let entries: Vec<Value> = errors
                .iter()
                .map(|e| {
                    let (field, expected) = e.field_and_expected();
                    let example = field.as_deref().and_then(field_example);
                    let mut entry = Map::new();
                    if let Some(field) = field {
                        entry.insert("field".to_string(), json!(field));
                    }
                    if let Some(expected) = expected {
                        entry.insert("expected".to_string(), json!(expected));
                    }
                    if let Some(example) = example {
                        entry.insert("example".to_string(), example);
                    }
                    Value::Object(entry)
                })
                .collect();
            obj.insert("errors".to_string(), Value::Array(entries));
        }
        // Requirement 2 / requirement 4: every rejection names the
        // quickstart tool that gives the caller a filled-in working
        // example for whatever it was trying to publish.
        // A payload that already names its own `docs` (`args_invalid` ->
        // `host.tool_test`) keeps it.
        obj.entry("docs".to_string()).or_insert(json!("host.quickstart"));
        // PRD-mcphost-onboarding-dead-ends-zero R3: the one place every
        // error gets its `next_call`; a payload naming its own keeps it.
        if !obj.contains_key("next_call") {
            let ctx = crate::next_call::ErrorCtx::from_error(&self);
            obj.insert("next_call".to_string(), self.next_call(&ctx).to_json());
        }
        // PRD-mcphost-first-hour-support-surface requirement 1 (AC1):
        // every payload carries its own correlation id, not just
        // `Internal`'s (which also folds it into `message` above, unchanged
        // from before this PRD).
        obj.insert("request_id".to_string(), json!(request_id));
        // PRD-mcphost-error-responses-invite-feedback R2/R4: the one place
        // every error gets its `feedback_hint`, generated from the code. The
        // tool is the one the error names (`ToolNotFound`) or `data.tool`.
        if !obj.contains_key("feedback_hint") {
            let tool = match &self {
                AppError::ToolNotFound(name) => Some(name.clone()),
                _ => obj.get("tool").and_then(Value::as_str).map(str::to_string),
            };
            let id = matches!(
                code,
                "internal" | "storage" | "registry_rejected" | "service_unavailable"
            )
            .then_some(request_id.as_str());
            obj.insert(
                "feedback_hint".to_string(),
                json!(crate::feedback::feedback_hint(code, tool.as_deref(), id)),
            );
        }
        // PRD-mcphost-errors-json-contract AC2: the registry row's own
        // `class`, `family` and `remedy` ride in every error's `data`, so a
        // client branches on them instead of matching the message.
        if let Some(row) = crate::error_registry::lookup(code) {
            obj.insert("class".to_string(), json!(row.class.as_str()));
            obj.insert("family".to_string(), json!(row.family.as_str()));
            obj.insert("remedy".to_string(), json!(row.remedy));
        }
        // AC1 / errors-json AC2: `help_url` for every registry code (and
        // every `crate::help::HELP_CODES` entry), resolving on whichever
        // base this call arrived on -- absent when no base was given (see
        // `into_error_data_at`'s doc comment).
        if let Some(base) = base_url
            && let Some(url) = crate::help::help_url(base, code)
        {
            obj.insert("help_url".to_string(), json!(url));
            // PRD-mcphost-emitted-doc-pointers-resolve R4: the same page's
            // doc section as an anchor `host.help.get` takes as-is.
            if let Some(entry) = crate::help::entry(code) {
                obj.insert("anchor".to_string(), json!(entry.doc_link.as_str()));
            }
        }
        crate::retry_policy::attach(&mut obj, code, attempt);
        crate::incident_banner::attach(&mut obj, code, base_url);
        error_envelope(jsonrpc_code, message, Value::Object(obj))
    }

    /// Requirement 3 / AC2: builds the single-field error exactly as before
    /// when [`Kind::validate_all`](crate::kinds::Kind::validate_all) found
    /// exactly one violation (unchanged wire shape -- every existing test
    /// asserting on it keeps passing), or an [`AppError::MultiInvalid`]
    /// carrying every violation at once when it found more than one, so
    /// `host.tool_publish` reports every failing field in a single
    /// rejection instead of one per attempt. `None` when `violations` is
    /// empty (the spec is valid).
    /// PRD-mcphost-sandbox-ready requirement 3 (AC3/AC7): a python-kind
    /// `host.tool_publish`/`host.tool_test`/`host.tool_run` on a host whose
    /// sandbox self-test is failing, rejected before the spec is ever
    /// evaluated -- never routed through `AppError::Internal` the way
    /// `KindError::Exec` normally maps (see `From<KindError>` below), which
    /// is exactly the bug this PRD's TL;DR describes: a real sandbox
    /// failure surfacing as an opaque internal error an agent retries
    /// uselessly. `data.docs` is filled in by `into_error_data`'s existing
    /// unconditional `"host.quickstart"` insert, same as every other
    /// rejection this crate returns -- not overridden here.
    /// PRD-mcphost-first-publish-real-kind requirement 3 (AC2): `retry_after_s`
    /// (the caller's own best current estimate, already clamped to `[1,
    /// 30]`) and `alternatives` now name only `http` -- `echo` is a stub
    /// (Goal 3), never a real fallback for a rejected python publish. The
    /// message itself now names the seconds too, so the hint survives a
    /// client that only surfaces `message`, not `data`.
    pub fn sandbox_unavailable(status: &crate::sandbox::SandboxStatus, retry_after_s: u32) -> Self {
        AppError::Structured {
            code: SANDBOX_UNAVAILABLE,
            message: format!(
                "the python kind is unavailable on this host; the spec was not evaluated -- \
                 retry in {retry_after_s}s or use http instead"
            ),
            data: json!({
                "mechanism": status.mechanism.as_str(),
                "detail": status.detail,
                "retry_after_s": retry_after_s,
                "alternatives": ["http"],
            }),
        }
    }

    /// PRD-mcphost-incident-banner-for-agents R1: the sandbox/exec backend is
    /// down -- a host fault, listed in [`HOST_FAULT_CODES`].
    pub fn exec_unavailable(detail: &str) -> Self {
        AppError::Structured { code: EXEC_UNAVAILABLE, message: format!("execution is unavailable: {detail}"), data: json!({}) }
    }

    /// R1: the host (not the caller's spec) ran out of time.
    pub fn host_timeout(detail: &str) -> Self {
        AppError::Structured { code: TIMEOUT, message: format!("the host timed out: {detail}"), data: json!({}) }
    }

    /// R1: the host as a whole is shedding load, as opposed to the per-tenant
    /// `rate_limited`.
    pub fn rate_limited_host() -> Self {
        AppError::Structured {
            code: RATE_LIMITED_HOST,
            message: "the host is shedding load; retry shortly".to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-python-kind-plain-env requirement 3 (AC4): a publish's
    /// own `env` name collides with a secret already known for this tenant
    /// -- secrets are tenant-scoped (not per-tool), so this is checked
    /// against every secret the tenant has ever set, not just ones this
    /// spec itself references.
    pub fn env_collides_with_secret(key: &str) -> Self {
        AppError::Structured {
            code: ENV_SECRET_COLLISION,
            message: format!(
                "env.{key}: '{key}' collides with an existing secret of the same name for this tenant"
            ),
            data: json!({"field": format!("env.{key}"), "key": key, "rule": "secret_collision"}),
        }
    }

    /// PRD-mcphost-python-kind-plain-env requirement 3 (AC4), the symmetric
    /// direction: `host.secret_set` refuses a new secret name that collides
    /// with any of this tenant's already-published tools' `env` entries.
    pub fn secret_collides_with_env(key: &str) -> Self {
        AppError::Structured {
            code: ENV_SECRET_COLLISION,
            message: format!(
                "'{key}' collides with a published tool's env entry of the same name"
            ),
            data: json!({"field": "name", "key": key, "rule": "env_collision"}),
        }
    }

    /// PRD-mcphost-tool-kind-honor requirement 1 (AC2): the caller's own
    /// requested `kind` disagrees with what the spec's shape can only be
    /// (see [`crate::kinds::infer::infer_kind_signal`]) -- refused before
    /// the spec is ever validated against (let alone stored under) the
    /// wrong `Kind` impl, naming the disagreeing element so the caller can
    /// fix either the spec or the request, instead of getting whichever
    /// kind's own `validate` happened to fail with a generic message (or,
    /// worse, silently succeed under the wrong kind).
    pub fn kind_mismatch(requested: &str, signal: &KindSignal) -> Self {
        AppError::Structured {
            code: KIND_MISMATCH,
            message: format!(
                "kind: requested '{requested}' but the spec's shape is only valid as '{}': {}",
                signal.kind, signal.reason
            ),
            data: json!({
                "requested": requested,
                "inferred": signal.kind,
                "reason": signal.reason,
            }),
        }
    }

    /// PRD-mcphost-agent-directory requirement 3 / AC3: a claim of a handle
    /// another tenant already holds -- names nothing about the holder, the
    /// whole point of the requirement.
    pub fn handle_taken() -> Self {
        AppError::Structured {
            code: HANDLE_TAKEN,
            message: "that handle is already claimed".to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-shared-tool-spec-readback requirement 4 (AC4): `host.
    /// tool_spec_shared` on a tool shared with the caller, but not with
    /// `expose_spec: true` -- distinct from [`AppError::ToolNotFound`]
    /// (requirement 4's other refusal, for a tool not shared with the
    /// caller at all), naming the owner namespace and tool name since the
    /// caller already knows this tool exists.
    pub fn spec_not_exposed(owner_ns: &str, name: &str) -> Self {
        AppError::Structured {
            code: SPEC_NOT_EXPOSED,
            message: format!("{owner_ns}.{name} has not exposed its spec"),
            data: json!({"tool": format!("{owner_ns}.{name}")}),
        }
    }

    /// Open questions: reserved handles (`admin`, `host`, `mcphost`,
    /// `system`), drafted as reserved -- released only via
    /// `admin.agent.handle_release`.
    pub fn handle_reserved(handle: &str) -> Self {
        AppError::Structured {
            code: HANDLE_RESERVED,
            message: format!("'{handle}' is a reserved handle and cannot be claimed"),
            data: json!({"handle": handle}),
        }
    }

    /// Requirement 4 / AC4: identical body for unknown, disabled, and
    /// deleted -- never lets a caller distinguish "doesn't exist" from
    /// "exists but hidden".
    pub fn agent_not_found() -> Self {
        AppError::Structured {
            code: AGENT_NOT_FOUND,
            message: "no agent found for that address".to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-agent-inbox requirement 3 / AC3: `host.msg.reply`/
    /// `host.msg.thread` on a thread the caller doesn't (or no longer)
    /// participate in -- byte-identical to a genuinely nonexistent thread
    /// id, same "unknown vs hidden" indistinguishability as
    /// [`Self::agent_not_found`] above.
    pub fn thread_not_found() -> Self {
        AppError::Structured {
            code: THREAD_NOT_FOUND,
            message: "no thread found for that id".to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-agent-inbox requirement 7: `msgs_per_hour`,
    /// `msg_body_bytes_max`, and `recipients_per_msg_max` all fail the
    /// whole `host.msg.send`/`reply` call with this shape (AC9/AC10:
    /// `data.limit` names the quota, `data.value` its numeric limit) --
    /// deliberately a flatter shape than `billing::quota_exceeded`'s
    /// `data.limit: {name, value}`, since the PRD pins the field names to
    /// exactly `limit`/`value` at the top level.
    pub fn msg_quota_exceeded(limit_name: &'static str, limit_value: i64) -> Self {
        AppError::Structured {
            code: QUOTA_EXCEEDED,
            message: format!("messaging quota exceeded: {limit_name} (limit {limit_value})"),
            data: json!({"limit": limit_name, "value": limit_value}),
        }
    }

    /// PRD-mcphost-agent-consent requirement 8: `host.msg.send`/`reply` to
    /// a `closed` recipient, or a `contacts`-mode one with no active
    /// request (AC1, AC8). `data.hint` names the tool that gets the caller
    /// unstuck (requirement 4) -- attached here rather than threaded
    /// through `db.rs`'s stored/dedup `refused` shape, since the hint is a
    /// fixed presentation of the code, not a stored fact (see
    /// `messaging.rs::send_outcome_json`, the other place this code can
    /// surface, for the same reasoning applied to a per-recipient refusal
    /// rather than this top-level one).
    pub fn contact_refused() -> Self {
        AppError::Structured {
            code: CONTACT_REFUSED,
            message: "that address does not accept messages from you yet".to_string(),
            data: json!({"hint": "host.agent.contact_request"}),
        }
    }

    /// PRD-mcphost-agent-consent requirement 3 (AC2, AC4): a request is
    /// already pending, or was denied within the last
    /// [`crate::consent::DENY_COOLDOWN_MS`] -- distinct from
    /// [`Self::contact_refused`] so a caller can tell "try again later"
    /// from "you need to ask first".
    pub fn contact_pending() -> Self {
        AppError::Structured {
            code: CONTACT_PENDING,
            message: "a contact request between you and that address is already pending or \
                was recently denied"
                .to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-agent-consent requirement 2: `host.agent.contact_request`
    /// against an `open` recipient, or one the caller already has an
    /// accepted contact with -- nothing to request.
    pub fn not_needed() -> Self {
        AppError::Structured {
            code: NOT_NEEDED,
            message: "no contact request is needed for that address".to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-agent-consent requirement 3: `host.agent.contact_accept`/
    /// `contact_deny` on a `request_id` that doesn't exist, isn't addressed
    /// to the caller, or is no longer `pending` -- collapsed to one code,
    /// same "unknown vs hidden" indistinguishability as
    /// [`Self::thread_not_found`].
    pub fn contact_request_not_found() -> Self {
        AppError::Structured {
            code: CONTACT_REQUEST_NOT_FOUND,
            message: "no pending contact request found for that id".to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-agent-mesh-ops requirement 4 / AC4: `admin.mesh.freeze`'s
    /// `mesh_frozen_at` is set. Distinct from [`AppError::TenantDisabled`]
    /// -- a frozen tenant's key still authenticates and its reads/tools/
    /// inbox keep working; only `host.msg.send`/`reply`,
    /// `host.channel.post` and `host.agent.contact_request` are refused.
    /// The reason given to `admin.mesh.freeze` is deliberately never
    /// echoed back here (requirement 4: "structured error with the reason
    /// omitted").
    pub fn mesh_frozen() -> Self {
        AppError::Structured {
            code: MESH_FROZEN,
            message: "this tenant is frozen by the operator; messaging and posting are paused"
                .to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-agent-mesh-ops: every `host.channel.*` verb against a key
    /// naming no channel it's allowed to resolve -- same "doesn't exist"
    /// shape as [`AppError::thread_not_found`]/[`AppError::agent_not_found`],
    /// plus (PRD-mcphost-channel-read-name-parity requirement 4 / AC4)
    /// `data.key` echoing the caller's own input and a message naming both
    /// lookups `resolve_channel` tried (`channels.rs`), so a miss explains
    /// itself instead of repeating the old "name or id" wording that named
    /// a lookup `read` never actually ran.
    pub fn channel_not_found(key: &str) -> Self {
        AppError::Structured {
            code: CHANNEL_NOT_FOUND,
            message: "no channel found for that key: tried group channel by id, and channel by id or name"
                .to_string(),
            data: json!({"key": key}),
        }
    }

    /// PRD-mcphost-agent-channels requirement 2 / AC9: `host.channel.post`
    /// against a channel the owner has closed -- reads keep working
    /// (`host.channel.read` never checks `closed_at`).
    pub fn channel_closed() -> Self {
        AppError::Structured {
            code: CHANNEL_CLOSED,
            message: "this channel is closed; posting is refused, reads still work".to_string(),
            data: json!({}),
        }
    }

    /// P1 requirement 10 / AC10: `host.channel.post` against a channel the
    /// owner has frozen -- same "reads still work" posture as
    /// [`Self::channel_closed`].
    pub fn channel_frozen() -> Self {
        AppError::Structured {
            code: CHANNEL_FROZEN,
            message: "this channel is frozen; posting is refused, reads still work".to_string(),
            data: json!({}),
        }
    }

    /// requirement 2/3 (AC7): `host.channel.open`/`host.channel.post` over
    /// a plan's `channels_max`/`channel_posts_per_hour` -- same flattened
    /// `data.limit`/`data.value` shape as [`Self::msg_quota_exceeded`],
    /// under the generic `quota_exceeded` code AC7 names.
    pub fn channel_quota_exceeded(limit_name: &'static str, limit_value: i64) -> Self {
        AppError::Structured {
            code: QUOTA_EXCEEDED,
            message: format!("channel quota exceeded: {limit_name} (limit {limit_value})"),
            data: json!({"limit": limit_name, "value": limit_value}),
        }
    }

    pub fn from_kind_violations(violations: Vec<KindError>) -> Option<AppError> {
        let mut iter = violations.into_iter();
        let first = AppError::from(iter.next()?);
        let rest: Vec<AppError> = iter.map(AppError::from).collect();
        if rest.is_empty() {
            return Some(first);
        }
        let mut errors = vec![first];
        errors.extend(rest);
        let fields: Vec<String> = errors
            .iter()
            .filter_map(|e| e.field_and_expected().0)
            .collect();
        let message = format!("{} fields invalid: {}", errors.len(), fields.join(", "));
        Some(AppError::MultiInvalid { message, errors })
    }

    /// PRD-mcphost-data-retention requirement 4 (AC6): `host.tool_call`,
    /// `host.tool_publish`, and `signup` all refuse with this before doing
    /// any write once free space on the database's filesystem drops below
    /// `$MCPHOST_DISK_FLOOR_MB` -- the PRD's own wire wording ("returns
    /// `service_unavailable: disk floor`") pins both the `code` and the
    /// leading words of `message`.
    /// PRD-mcphost-python-dependency-policy requirement 3 (AC5): a
    /// `requirements` entry that is denylisted, or within
    /// Damerau-Levenshtein distance 1 of a top-100 PyPI package name (and
    /// isn't that name itself) -- refused before any network resolution is
    /// attempted. The PRD's own AC pins the message text verbatim
    /// (`dependency_policy: denied (near-name of requests)`).
    pub fn dependency_policy_denied(reason: &str) -> Self {
        AppError::Structured {
            code: DEPENDENCY_POLICY,
            message: format!("dependency_policy: denied ({reason})"),
            data: json!({"reason": reason}),
        }
    }

    /// requirement 2 (AC3): a pinned requirement (or a supplied lock's own
    /// pin) matches a known advisory with a fix available, and
    /// `MCPHOST_ADVISORY_MODE=fail` is set -- the PRD's own AC pins the
    /// message text verbatim (`dependency_advisory: <id> <package>
    /// fix=<version>`).
    pub fn dependency_advisory(id: &str, package: &str, fixed: &str) -> Self {
        AppError::Structured {
            code: DEPENDENCY_ADVISORY,
            message: format!("dependency_advisory: {id} {package} fix={fixed}"),
            data: json!({"advisory_id": id, "package": package, "fixed": fixed}),
        }
    }

    /// PRD-mcphost-sandbox-egress-allowlist requirement 1 (AC1/AC2): a
    /// `network: "public"`/`"egress"` spec published by a tenant whose plan
    /// isn't `plan` -- shared wording (via `network_policy::
    /// plan_required_fields`) with the run-time refusal
    /// `kinds::python::PythonKind::network_mode` raises for requirement 3
    /// (AC6), so an agent sees the same `code`/message/republish hint
    /// whichever path refused it.
    pub fn plan_required(field: &str, plan: &str) -> Self {
        let (message, data) = crate::network_policy::plan_required_fields(field, plan);
        AppError::Structured {
            code: crate::subkeys::PLAN_REQUIRED,
            message,
            data,
        }
    }

    /// PRD-mcphost-sandbox-resource-limits-from-plan R3: a spec value
    /// (`spec_field`, e.g. `memory_mb`) above the tenant's plan ceiling
    /// (`ceiling_field`, e.g. `sandbox_memory_mb_max`). Same `plan_required`
    /// code as the network refusal; `data` names the ceiling and its value.
    pub fn plan_ceiling_exceeded(
        spec_field: &str,
        requested: i64,
        ceiling_field: &str,
        ceiling: i64,
        plan: &str,
    ) -> Self {
        AppError::Structured {
            code: crate::subkeys::PLAN_REQUIRED,
            message: format!(
                "{spec_field}: {requested} exceeds {ceiling_field} {ceiling} on the {plan} plan -- lower {spec_field} to {ceiling} or less, or upgrade"
            ),
            data: json!({
                "plan": plan,
                "field": ceiling_field,
                "spec_field": spec_field,
                "limit": ceiling,
                "requested": requested,
            }),
        }
    }

    /// requirement 1: `uv pip compile --generate-hashes` failed, or a
    /// caller-supplied lock (requirement 5) failed hash validation -- `tail`
    /// is the resolver's own capped tail, same convention
    /// `kinds::python::run_command_tail` uses for a failed env build.
    pub fn dependency_resolve_failed(tail: &str) -> Self {
        AppError::Structured {
            code: DEPENDENCY_RESOLVE_FAILED,
            message: format!("dependency resolution failed: {tail}"),
            data: json!({"tail": tail}),
        }
    }

    /// PRD-mcphost-signup-kill-switch-and-source requirement 3 / AC4:
    /// `signup` while the pause file exists. `message` is the file's own
    /// first line (or [`crate::state::SIGNUP_PAUSE_DEFAULT_MESSAGE`]) --
    /// carried verbatim as this error's own `message` (the
    /// `#[error("{message}")]` on [`AppError::Structured`] above), not
    /// wrapped in extra prose, so AC4's "message equals the file's first
    /// line" holds byte-for-byte.
    pub fn signup_paused(message: String, retry_after_secs: i64) -> Self {
        AppError::Structured {
            code: SIGNUP_PAUSED,
            message,
            data: json!({"retry_after_secs": retry_after_secs}),
        }
    }

    /// PRD-mcphost-first-call-gift requirement 1 (AC2): `signup`/
    /// `host.quickstart`'s own `remember` argument over the 4096-byte cap --
    /// never partially stored (checked before the tenant/state row would be
    /// written).
    pub fn remember_too_long(len: usize) -> Self {
        AppError::Structured {
            code: REMEMBER_TOO_LONG,
            message: format!("remember must be at most 4096 bytes; got {len}"),
            data: json!({"max_bytes": 4096, "len": len}),
        }
    }

    /// PRD-mcphost-implicit-signup requirement 3 (AC3): the implicit-signup
    /// path's own rate-limit refusal -- a distinct code from the explicit
    /// `signup` tool's plain [`Self::RateLimited`], since an anonymous
    /// `host.*`/`billing.*` call that trips the limiter never asked to sign
    /// up at all. `data.help` points at the human `/u/new` page and
    /// `data.retry_after_s` is the limiter's own rolling window -- the
    /// caller's own best current estimate of when a slot frees up again.
    pub fn signup_rate_limited(public_url: &str, retry_after_s: i64) -> Self {
        AppError::Structured {
            code: SIGNUP_RATE_LIMITED,
            message: "too many tenants created from this address in the last hour".to_string(),
            data: json!({
                "help": format!("{}/u/new", public_url.trim_end_matches('/')),
                "retry_after_s": retry_after_s,
            }),
        }
    }

    /// PRD-mcphost-abuse-guard-ban-list requirement 2 / AC1-3: a banned
    /// subject's refusal -- `expires_at` always present (`null` for a
    /// permanent ban), `reason` present as a key only when the operator
    /// marked the ban `public: true` (AC3): the `data` object omits the
    /// key entirely rather than carrying it as `null`, so a caller checking
    /// `"reason" in data` (not just its value) still sees "no reason" for a
    /// private ban.
    pub fn banned(expires_at: Option<i64>, reason: Option<String>) -> Self {
        let mut data = Map::new();
        data.insert("expires_at".to_string(), json!(expires_at));
        if let Some(reason) = reason {
            data.insert("reason".to_string(), json!(reason));
        }
        AppError::Structured {
            code: crate::deadletters::BANNED,
            message: "this subject is banned".to_string(),
            data: Value::Object(data),
        }
    }

    pub fn disk_floor(free_bytes: u64, floor_bytes: u64) -> Self {
        AppError::Structured {
            code: SERVICE_UNAVAILABLE,
            message: format!(
                "disk floor: {free_bytes} bytes free is below the {floor_bytes}-byte floor"
            ),
            data: json!({
                "reason": "disk_floor",
                "free_bytes": free_bytes,
                "floor_bytes": floor_bytes,
            }),
        }
    }

    /// PRD-mcphost-ownership-moment requirement 3 (AC3): the claim magic
    /// link's own send failure -- a non-2xx from the provider (`status`
    /// carries its HTTP status) or a pure transport failure (`status:
    /// None`, DNS/connect/timeout). `message`/`data` never carry the
    /// provider's response body or `MCPHOST_EMAIL_API_KEY` -- only the
    /// status code, same "name the fact, not the detail" posture
    /// [`Self::disk_floor`] already takes.
    pub fn claim_email_failed(status: Option<u16>) -> Self {
        AppError::Structured {
            code: CLAIM_EMAIL_FAILED,
            message: "could not send the claim email".to_string(),
            data: json!({"status": status}),
        }
    }

    /// The companion read for [`Self::claim_email_failed`]: the provider
    /// status code it carried, for a journal write -- `None` for every
    /// other variant, and for a transport failure (no status to report).
    pub fn claim_email_status(&self) -> Option<u16> {
        match self {
            AppError::Structured { code: CLAIM_EMAIL_FAILED, data, .. } => {
                data.get("status").and_then(Value::as_u64).map(|v| v as u16)
            }
            _ => None,
        }
    }

    /// PRD-mcphost-shared-tool-call-path requirement 2 (AC2, AC3): the
    /// cross-tenant `<owner_ns>.<local_name>` not-found shape --
    /// [`crate::sharing::resolve_shared_tool`]'s only error, identical bytes
    /// for every miss reason (owner namespace doesn't exist, tool doesn't
    /// exist, tool is private, or caller isn't in the sharing group) so a
    /// probe can never learn which of those is true from the message, the
    /// data, or (same one-or-two-query shape either way) response timing.
    pub fn shared_tool_not_found(owner_ns: &str, local_name: &str) -> Self {
        AppError::Structured {
            code: TOOL_NOT_FOUND,
            message: format!("tool not found: {owner_ns}.{local_name}"),
            data: json!({
                "hint": "shared tools are called as <owner_namespace>.<tool>; ask the owner \
                    to host.tool_share it with you or your group",
            }),
        }
    }

    /// PRD-mcphost-invite-links requirement 1 (AC7): `host.invite.create`
    /// past the plan's live-standard-invites cap. `limit` is named
    /// `invites_max` (not the generic `limit`/`used` shape
    /// `ShareQuotaExceeded` uses) because the PRD's own AC7 pins that exact
    /// field name.
    pub fn invites_quota_exceeded(limit: i64, used: i64) -> Self {
        AppError::Structured {
            code: INVITES_QUOTA_EXCEEDED,
            message: format!("tenant already has {used} live invites, the plan maximum of {limit}"),
            data: json!({"invites_max": limit, "used": used}),
        }
    }

    /// PRD-mcphost-invite-links requirement 4 (AC5) / requirement 3 (AC4):
    /// a code that doesn't resolve to a live invite at all -- unknown,
    /// revoked, expired, or (the AC4 race) already at `max_uses`. One code
    /// for every miss reason, same non-leaking posture as
    /// [`Self::shared_tool_not_found`]: never distinguishes "never existed"
    /// from "existed once" from "the last slot was just taken".
    pub fn invite_invalid() -> Self {
        AppError::Structured {
            code: INVITE_INVALID,
            message: "this invite link is no longer valid".to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-same-user-second-agent R3 (AC2): a join code that is
    /// unknown, revoked, expired or used up. `next_call` points the caller
    /// at `host.whoami` on any sibling, which prints the live code.
    pub fn join_code_invalid() -> Self {
        AppError::Structured {
            code: JOIN_CODE_INVALID,
            message: "this join link is no longer valid; ask a client already in the workspace \
                      for a fresh one (host.whoami prints the live join_url)"
                .to_string(),
            data: json!({"next_call": {"tool": "host.whoami", "args": {}}}),
        }
    }

    /// PRD-mcphost-same-user-second-agent R4 (AC4): `host.client.revoke` on
    /// the only live client would leave the tenant with no credential.
    pub fn last_client() -> Self {
        AppError::Structured {
            code: LAST_CLIENT,
            message: "this is the only client of the workspace; revoking it would lock everyone out"
                .to_string(),
            data: json!({"next_call": {"tool": "host.whoami", "args": {}}}),
        }
    }

    /// `host.client.revoke` for a `client_id` that is not a live client of
    /// the caller's tenant.
    pub fn client_not_found(client_id: &str) -> Self {
        AppError::Structured {
            code: crate::oauth_policy::CLIENT_NOT_FOUND,
            message: format!("no live client '{client_id}' in this workspace"),
            data: json!({"client_id": client_id, "next_call": {"tool": "host.whoami", "args": {}}}),
        }
    }

    /// PRD-mcphost-invite-links requirement 3 / 11 (AC6, AC10): the
    /// per-code hourly join limiter -- distinct from [`Self::invite_invalid`]
    /// so a client can tell "come back later" from "this link is dead".
    pub fn invite_rate_limited() -> Self {
        AppError::Structured {
            code: INVITE_RATE_LIMITED,
            message: "this invite has reached its hourly join limit; try again later".to_string(),
            data: json!({}),
        }
    }

    /// PRD-mcphost-tool-naming-convention-and-aliases requirement 6 (AC6):
    /// `dispatch_tenant_tool`'s own `tool_not_found` for an unmatched
    /// `host.*`/`billing.*` name -- `data.did_you_mean` names every
    /// *canonical* candidate (never a deprecated alias: steering a typo
    /// toward another old name would defeat this PRD's own point) within
    /// [`MAX_DID_YOU_MEAN_DISTANCE`] edits of `requested`, nearest first,
    /// capped at [`MAX_DID_YOU_MEAN`] -- same thresholds `did_you_mean`
    /// (above) already uses for `UnknownKind`, reimplemented here rather
    /// than reused because that function's signature is pinned to
    /// `&'static str` candidates (a small, static kind registry); the
    /// control-plane tool registry is read fresh off `AppState` per call,
    /// so `candidates` arrives as owned `String`s instead.
    /// PRD-mcphost-tool-call-host-verb-forward requirement 2 (AC4): a
    /// `host.tool_call`/`host.tool_test` name this host will never forward --
    /// an `admin.*`/`billing.*` name (Non-goals: never forwarded, whatever
    /// the registry says) or one of `verbforward::FORWARD_DENYLIST`'s own
    /// four. `host_verb` is `name` exactly as the caller sent it (for
    /// `admin.*`/`billing.*`, which this host never resolves against the
    /// verb registry at all) or the normalizer's own canonical dotted form
    /// (for a denylisted real verb); `call_instead` always names the
    /// client-visible (underscore) tool and echoes back the same `args`
    /// the caller already sent, so the hint is a call the caller can make
    /// verbatim.
    pub fn host_verb_not_forwarded(host_verb: &str, call_args: &Value) -> Self {
        let client_tool = host_verb.replace('.', "_");
        AppError::Structured {
            code: TOOL_NOT_FOUND,
            message: format!("tool not found: {host_verb}"),
            data: json!({
                "host_verb": host_verb,
                "call_instead": {"tool": client_tool, "args": call_args},
                "hint": format!(
                    "{host_verb} is not forwarded through host.tool_call/host.tool_test -- call {client_tool} directly"
                ),
            }),
        }
    }

    ///
    /// PRD-mcphost-tools-list-alias-truth requirement 3: the same error
    /// also carries the `unknown_tool` shape -- `error`, `name`, and
    /// `nearest`, up to [`MAX_DID_YOU_MEAN`] names from `advertised` (the
    /// exact `tools/list` set, aliases and flattened forms included) by
    /// edit distance with no distance cutoff, so a caller is never left
    /// with an empty hint. Advice only: nothing is auto-corrected.
    pub fn host_tool_not_found(requested: &str, candidates: &[String], advertised: &[String]) -> Self {
        let lower = requested.to_ascii_lowercase();
        let mut scored: Vec<(usize, &str)> = candidates
            .iter()
            .map(|c| (levenshtein(&lower, &c.to_ascii_lowercase()), c.as_str()))
            .filter(|(d, _)| *d <= MAX_DID_YOU_MEAN_DISTANCE)
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
        scored.truncate(MAX_DID_YOU_MEAN);
        let did_you_mean: Vec<&str> = scored.into_iter().map(|(_, c)| c).collect();
        let mut near: Vec<(usize, &str)> =
            advertised.iter().map(|c| (levenshtein(&lower, &c.to_ascii_lowercase()), c.as_str())).collect();
        near.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
        let nearest: Vec<&str> = near.into_iter().take(MAX_DID_YOU_MEAN).map(|(_, c)| c).collect();
        AppError::Structured {
            code: TOOL_NOT_FOUND,
            message: format!("unknown tool {requested}; nearest: {}", nearest.join(", ")),
            data: json!({
                "error": "unknown_tool",
                "name": requested,
                "nearest": nearest,
                "did_you_mean": did_you_mean,
            }),
        }
    }
}

impl From<KindError> for AppError {
    fn from(e: KindError) -> Self {
        match e {
            KindError::InvalidSpec(m) => AppError::InvalidSpec(m),
            KindError::InvalidArgs(m) => AppError::InvalidArgs(m),
            KindError::Structured {
                code,
                message,
                data,
            } => AppError::Structured {
                code,
                message,
                data,
            },
            KindError::Exec(m) => AppError::Internal(m),
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        // PRD-mcphost-sqlite-busy-timeout-audit requirement 3: the one
        // place every rusqlite error in this crate passes through, so
        // this is where SQLITE_BUSY/SQLITE_LOCKED get counted -- the wire
        // error stays exactly what it was (AC5: "the caller receives the
        // existing structured error").
        crate::db::note_rusqlite_error(&e);
        if let rusqlite::Error::SqliteFailure(f, msg) = &e
            && f.code == rusqlite::ffi::ErrorCode::ConstraintViolation
        {
            return constraint_conflict(f.extended_code, msg.as_deref().unwrap_or(""));
        }
        // PRD-mcphost-dry-run-inside-savepoint-for-transactional-verbs R5:
        // the one refusal SQLite gives a raw `BEGIN` inside an open
        // transaction. rusqlite 0.32 has no `ErrorCode` for SQLITE_ERROR
        // (it falls to `Unknown`), so both the extended code and the pinned
        // text must match; every other rusqlite error stays `storage`.
        if let rusqlite::Error::SqliteFailure(f, Some(msg)) = &e
            && f.extended_code == rusqlite::ffi::SQLITE_ERROR
            && msg == NESTED_TRANSACTION_SQLITE_MESSAGE
        {
            return AppError::Structured {
                code: TRANSACTION_NESTED,
                message: "a write verb tried to open a transaction inside one that was already open; \
                          retrying fails the same way"
                    .to_string(),
                data: json!({}),
            };
        }
        AppError::Storage(e.to_string())
    }
}

/// PRD-mcphost-table-conflict-structured-error: a SQLite constraint violation
/// is the caller's data, never a host fault. The extended result code names
/// the constraint; SQLite's stable `<KIND> constraint failed: <table>.<column>`
/// text supplies table and column (either may be absent, e.g. a CHECK).
fn constraint_conflict(extended_code: i32, sqlite_msg: &str) -> AppError {
    let constraint = match extended_code {
        2067 => "unique",
        1555 => "primary_key",
        1299 => "not_null",
        275 => "check",
        _ => "other",
    };
    let target = sqlite_msg
        .split_once("constraint failed: ")
        .map(|(_, t)| t.split(',').next().unwrap_or(t).trim())
        .unwrap_or("");
    let (table, column) = match target.rsplit_once('.') {
        Some((t, c)) => (Some(t), Some(c)),
        None => (None, None),
    };
    let on = column.map(|c| format!(" on {c}")).unwrap_or_default();
    AppError::Structured {
        code: crate::tables::TABLE_ROW_CONFLICT,
        message: format!("row conflicts{on} ({constraint})"),
        data: serde_json::json!({"constraint": constraint, "table": table, "column": column}),
    }
}

/// The text bundled SQLite gives a `BEGIN` inside a transaction, pinned
/// against the bundled library by `dryspt_ac06`.
pub const NESTED_TRANSACTION_SQLITE_MESSAGE: &str = "cannot start a transaction within a transaction";

crate::err_code! {
    pub TRANSACTION_NESTED => "transaction_nested",
    Storage, HostFault, false,
    "Retrying fails the same way; report data.request_id through the support link.",
}

crate::err_code! {
    pub AGENT_NOT_FOUND,
    code: "agent_not_found", family: Share, class: AgentFault, retryable: false,
    remedy: "Check the address with host.agent.lookup, then re-send to a handle that exists.",
}

crate::err_code! {
    pub CHANNEL_CLOSED,
    code: "channel_closed", family: Share, class: AgentFault, retryable: false,
    remedy: "Posting is refused on a closed channel; reads still work. Open a new channel to continue.",
}

crate::err_code! {
    pub CHANNEL_FROZEN,
    code: "channel_frozen", family: Share, class: AgentFault, retryable: false,
    remedy: "Posting is refused on a frozen channel; reads still work. Ask the owner to unfreeze it.",
}

crate::err_code! {
    pub CHANNEL_NOT_FOUND,
    code: "channel_not_found", family: Share, class: AgentFault, retryable: false,
    remedy: "Check data.key against host.channel.list and use a channel id or name that exists.",
}

crate::err_code! {
    pub CLAIM_EMAIL_FAILED,
    code: "claim_email_failed", family: Internal, class: HostFault, retryable: true,
    remedy: "The claim email could not be sent; retry later or use the claim link from the response.",
}

crate::err_code! {
    pub CONTACT_PENDING,
    code: "contact_pending", family: Share, class: AgentFault, retryable: false,
    remedy: "A request is already pending or recently denied; wait for the recipient to answer.",
}

crate::err_code! {
    pub CONTACT_REFUSED,
    code: "contact_refused", family: Share, class: HostRefusal, retryable: false,
    remedy: "Send host.agent.contact_request first and wait for acceptance.",
}

crate::err_code! {
    pub CONTACT_REQUEST_NOT_FOUND,
    code: "contact_request_not_found", family: Share, class: AgentFault, retryable: false,
    remedy: "List pending requests and use an id that exists.",
}

crate::err_code! {
    pub DEPENDENCY_ADVISORY,
    code: "dependency_advisory", family: Spec, class: HostRefusal, retryable: false,
    remedy: "Pin the package to data.fixed or later and republish.",
}

crate::err_code! {
    pub DEPENDENCY_POLICY,
    code: "dependency_policy", family: Spec, class: HostRefusal, retryable: false,
    remedy: "Remove or replace the requirement named in data.reason; the policy denies it.",
}

crate::err_code! {
    pub DEPENDENCY_RESOLVE_FAILED,
    code: "dependency_resolve_failed", family: Spec, class: AgentFault, retryable: false,
    remedy: "Read data.tail for the resolver error and fix the requirements.",
}

crate::err_code! {
    pub ENV_SECRET_COLLISION,
    code: "env_secret_collision", family: Spec, class: AgentFault, retryable: false,
    remedy: "Rename the env entry or the secret so no name is used by both.",
}

crate::err_code! {
    pub EXEC_UNAVAILABLE,
    code: "exec_unavailable", family: Internal, class: HostFault, retryable: true,
    remedy: "The sandbox or exec backend is down; retry later or use a kind that does not need it.",
}

crate::err_code! {
    pub HANDLE_RESERVED,
    code: "handle_reserved", family: Share, class: AgentFault, retryable: false,
    remedy: "Choose a different handle; this one is reserved.",
}

crate::err_code! {
    pub HANDLE_TAKEN,
    code: "handle_taken", family: Share, class: AgentFault, retryable: false,
    remedy: "Choose a different handle; this one is already claimed.",
}

crate::err_code! {
    pub INVITE_INVALID,
    code: "invite_invalid", family: Auth, class: HostRefusal, retryable: false,
    remedy: "Ask the owner for a new invite link; this one is no longer valid.",
}

crate::err_code! {
    pub INVITE_RATE_LIMITED,
    code: "invite_rate_limited", family: Plan, class: HostRefusal, retryable: true,
    remedy: "Try the invite link again later; it reached its hourly join limit.",
}

crate::err_code! {
    pub INVITES_QUOTA_EXCEEDED,
    code: "invites_quota_exceeded", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Revoke an unused invite or move to a plan with a higher invites_max.",
}

crate::err_code! {
    pub JOIN_CODE_INVALID,
    code: "join_code_invalid", family: Auth, class: HostRefusal, retryable: false,
    remedy: "Ask a client already in the workspace for a fresh join link; host.whoami (data.next_call) prints the live join_url.",
}

crate::err_code! {
    pub KIND_MISMATCH,
    code: "kind_mismatch", family: Spec, class: AgentFault, retryable: false,
    remedy: "Send the kind the spec's shape needs (data.inferred), or change the spec to match data.requested.",
}

crate::err_code! {
    pub LAST_CLIENT,
    code: "last_client", family: Auth, class: AgentFault, retryable: false,
    remedy: "Add another client to the workspace first (host.whoami prints the join_url), then revoke this one.",
}

crate::err_code! {
    pub MESH_FROZEN,
    code: "mesh_frozen", family: Share, class: HostRefusal, retryable: false,
    remedy: "Messaging and posting are paused by the operator for this tenant; do not retry.",
}

crate::err_code! {
    pub NOT_NEEDED,
    code: "not_needed", family: Share, class: AgentFault, retryable: false,
    remedy: "Send the message directly; no contact request is needed.",
}

crate::err_code! {
    pub QUOTA_EXCEEDED,
    code: "quota_exceeded", family: Plan, class: PlanLimit, retryable: false,
    remedy: "Read data for the exceeded quota; reduce usage or move to a plan with a higher limit.",
}

crate::err_code! {
    pub RATE_LIMITED_HOST,
    code: "rate_limited_host", family: Internal, class: HostFault, retryable: true,
    remedy: "The host is shedding load; retry shortly.",
}

crate::err_code! {
    pub REMEMBER_TOO_LONG,
    code: "remember_too_long", family: Args, class: AgentFault, retryable: false,
    remedy: "Shorten remember to 4096 bytes or fewer.",
}

crate::err_code! {
    pub SANDBOX_UNAVAILABLE,
    code: "sandbox_unavailable", family: Internal, class: HostFault, retryable: true,
    remedy: "Retry in data.retry_after_s seconds or use http instead; the python kind is unavailable.",
}

crate::err_code! {
    pub SERVICE_UNAVAILABLE,
    code: "service_unavailable", family: Internal, class: HostFault, retryable: true,
    remedy: "Retry later; the host is out of a resource such as disk space.",
}

crate::err_code! {
    pub SIGNUP_PAUSED,
    code: "signup_paused", family: Auth, class: HostRefusal, retryable: true,
    remedy: "Retry after data.retry_after_secs; signup is paused.",
}

crate::err_code! {
    pub SIGNUP_RATE_LIMITED,
    code: "signup_rate_limited", family: Plan, class: HostRefusal, retryable: true,
    remedy: "Retry after data.retry_after_s; too many tenants were created from this address.",
}

crate::err_code! {
    pub SPEC_NOT_EXPOSED,
    code: "spec_not_exposed", family: Share, class: HostRefusal, retryable: false,
    remedy: "Ask the owner to expose the spec; it is not shared.",
}

crate::err_code! {
    pub THREAD_NOT_FOUND,
    code: "thread_not_found", family: Share, class: AgentFault, retryable: false,
    remedy: "List threads and use an id that exists.",
}

crate::err_code! {
    pub TIMEOUT,
    code: "timeout", family: Internal, class: HostFault, retryable: true,
    remedy: "The host timed out; retry shortly.",
}

crate::err_code! {
    pub UNKNOWN_ARGUMENT,
    code: "unknown_argument", family: Args, class: AgentFault, retryable: false,
    remedy: "Remove the argument; data.known lists the recognised ones.",
}


#[cfg(test)]
mod internal_error_tests {
    use super::*;

    /// Audit finding 2 (mcphost-polish-p0-20260930): a `billing.rs`/
    /// `authz.rs` call site that wraps raw upstream text (Stripe API
    /// bodies, `io::Error`/pkcs8 decode text, ...) into `AppError::Internal`
    /// must never hand that text to the client -- only a generic message
    /// plus a correlation id the server log carries the real detail under.
    #[test]
    fn internal_error_response_hides_detail_and_carries_request_id() {
        let detail = "stripe: secret-ish detail";
        let response = AppError::Internal(detail.to_string()).into_error_data();

        assert!(
            !response.message.contains(detail),
            "client-facing message must not contain the wrapped detail: {}",
            response.message
        );
        assert!(
            response.message.contains("request_id="),
            "client-facing message must carry a request_id: {}",
            response.message
        );
        // The machine-readable code string is unchanged by this fix.
        let data = response.data.expect("error data object");
        assert_eq!(data["error_code"], "internal");
        assert!(
            !data.to_string().contains(detail),
            "the structured error data must not carry the wrapped detail either: {data}"
        );
    }
}
