//! The `http` kind: a tenant publishes a declarative REST-API wrapper
//! (method, URL/header/query/body templates, an `args_schema`) and the host
//! calls it on the agent's behalf, injecting the tenant's secrets and
//! enforcing SSRF, template-strictness and size/time bounds along the way.
//! PRD: `PRD-mcphost-rest-tools.md`.
//!
//! Two defenses close the SSRF surface (PRD requirement 3 / 8, AC2 / AC9):
//! * **Publish time** (`validate`): the spec's literal (pre-render) `url`
//!   host is checked syntactically against private/loopback/link-local
//!   ranges, `.internal`, and the host's own domain -- cheap and catches the
//!   lazy/obvious cases before a tool ever exists.
//! * **Call time** (`call` + [`VettingResolver`]): the *rendered* url is
//!   re-checked (covering a templated host driven by call arguments), and
//!   for an actual hostname (not a literal IP) DNS resolution happens
//!   through a custom `reqwest::dns::Resolve` backed by `hickory-resolver`
//!   that rejects any resolved address in a disallowed range *before*
//!   `reqwest` ever connects to it -- the checked address is the address
//!   connected to, closing the classic check-then-connect DNS-rebinding
//!   window.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::infer;
use super::{CallCtx, Kind, KindError, KindExample, ToolDescriptor};

const DEFAULT_TIMEOUT_S: u64 = 10;
const MAX_TIMEOUT_S: u64 = 30;
const DEFAULT_RATE_LIMIT_PER_MINUTE: u32 = 600;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
const ALLOWED_METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
/// Response header names ever surfaced back to the caller (Technical
/// Considerations: "Response header subset returned").
const RESPONSE_HEADER_SUBSET: [&str; 3] = ["content-type", "retry-after", "x-request-id"];

// ---- spec ----------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct HttpSpec {
    method: String,
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    query: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
    /// Requirement 1: optional -- when absent,
    /// [`infer::infer_http_args_schema`] derives it from the placeholders
    /// referenced across `url`/`headers`/`query`/`body` (requirement 4).
    /// When present, used exactly as before with no inference performed.
    #[serde(default)]
    args_schema: Option<Value>,
    #[serde(default)]
    timeout_s: Option<u64>,
    #[serde(default)]
    response: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

impl HttpSpec {
    fn effective_timeout_s(&self) -> u64 {
        self.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S)
    }

    fn effective_response(&self) -> &str {
        self.response.as_deref().unwrap_or("json")
    }

    /// Requirement 1/4: the schema to validate calls against -- the
    /// author's own, unchanged, or one derived from every placeholder in
    /// `url`/`headers`/`query`/`body` when absent. Deterministic (AC13): a
    /// pure function of the spec's own templates.
    fn effective_args_schema(&self) -> Result<Value, KindError> {
        match &self.args_schema {
            Some(schema) => Ok(schema.clone()),
            None => infer::infer_http_args_schema(&template_fields(self)),
        }
    }
}

fn parse_spec(spec: &Value) -> Result<HttpSpec, KindError> {
    if !spec.is_object() {
        return Err(KindError::InvalidSpec("spec: must be a JSON object".into()));
    }
    serde_json::from_value(spec.clone()).map_err(|e| KindError::InvalidSpec(format!("spec: {e}")))
}

/// Every template string in `spec`, paired with a dotted field path used in
/// error messages (`"headers.Authorization"`, `"body.customer.id"`, ...).
fn template_fields(spec: &HttpSpec) -> Vec<(String, String)> {
    let mut out = vec![("url".to_string(), spec.url.clone())];
    for (k, v) in &spec.headers {
        out.push((format!("headers.{k}"), v.clone()));
    }
    for (k, v) in &spec.query {
        out.push((format!("query.{k}"), v.clone()));
    }
    if let Some(body) = &spec.body {
        collect_body_templates("body".to_string(), body, &mut out);
    }
    out
}

fn collect_body_templates(path: String, value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::String(s) => out.push((path, s.clone())),
        Value::Object(map) => {
            for (k, v) in map {
                collect_body_templates(format!("{path}.{k}"), v, out);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                collect_body_templates(format!("{path}[{i}]"), v, out);
            }
        }
        _ => {}
    }
}

fn referenced_secrets_in_spec(spec: &HttpSpec) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (_, template) in template_fields(spec) {
        out.extend(extract_secret_refs(&template));
    }
    out
}

// ---- template variable scanning -------------------------------------------
//
// Deliberately not a full Jinja parser: this project's templates are single
// `{{ name }}` / `{{ name | filter }}` / `{{ secret.name }}` expressions
// (requirement 2's variables are exactly "the call arguments and
// `secret.<name>`"), so a scanner that extracts each `{{ ... }}` block's
// leading dotted identifier is enough to (a) know which secrets a spec
// needs at publish time and (b) name the exact undefined variable in a
// `template_error` at call time, without depending on minijinja's own error
// detail format.

/// Every `{{ ... }}` expression's trimmed inner text, in order.
fn iter_expressions(template: &str) -> impl Iterator<Item = &str> {
    let mut rest = template;
    std::iter::from_fn(move || {
        let start = rest.find("{{")?;
        let after_open = &rest[start + 2..];
        let end = after_open.find("}}")?;
        let expr = after_open[..end].trim();
        rest = &after_open[end + 2..];
        Some(expr)
    })
}

/// The leading `identifier` or `identifier.identifier` an expression's
/// value comes from -- `name` for `name | urlencode`, `secret.stripe` for
/// `secret.stripe | default('x')`.
fn root_reference(expr: &str) -> Option<String> {
    let bytes = expr.as_bytes();
    if bytes
        .first()
        .is_none_or(|b| !(b.is_ascii_alphabetic() || *b == b'_'))
    {
        return None;
    }
    let mut i = 0;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    let mut name = expr[..i].to_string();
    if bytes.get(i) == Some(&b'.') {
        let dot_start = i + 1;
        let mut j = dot_start;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        if j > dot_start {
            name.push('.');
            name.push_str(&expr[dot_start..j]);
        }
    }
    Some(name)
}

fn extract_secret_refs(template: &str) -> BTreeSet<String> {
    iter_expressions(template)
        .filter_map(root_reference)
        .filter_map(|r| r.strip_prefix("secret.").map(str::to_string))
        .collect()
}

/// The first reference in `template` that isn't in `known` -- the exact
/// undefined variable a `template_error` should name (AC8).
fn find_undefined_ref(template: &str, known: &BTreeSet<String>) -> Option<String> {
    iter_expressions(template)
        .filter_map(root_reference)
        .find(|r| !known.contains(r))
}

// ---- URL / host checks -----------------------------------------------------

/// Splits `scheme://host[:port][/...]` into `(scheme, host)`. No claim to
/// full RFC 3986 correctness -- templated paths can contain raw `{{ }}`
/// text the `url` crate would balk at, so this only needs to be right about
/// the scheme and authority's host component.
fn split_scheme_host(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(stripped)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    Some((scheme, host))
}

/// `own_domain` stripped of scheme/port, for the "host's own domain" SSRF
/// rule (requirement 3). Shared by `main.rs` (production) and the test
/// harness (against the ephemeral test server's own `base_url`).
pub fn own_domain_from_url(url: &str) -> String {
    split_scheme_host(url)
        .map(|(_, host)| host.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_ipv6_link_local(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn is_ipv6_unique_local(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// Private / loopback / link-local / unspecified / documentation ranges.
/// `allow_loopback` is a test-only escape hatch (see [`HttpKind::for_test`])
/// -- the production constructor never sets it, so `127.0.0.1` stays
/// blocked for every real deployment.
fn is_disallowed_ip(ip: IpAddr, allow_loopback: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            (!allow_loopback && v4.is_loopback())
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
        }
        IpAddr::V6(v6) => {
            (!allow_loopback && v6.is_loopback())
                || v6.is_unspecified()
                || v6.is_multicast()
                || is_ipv6_link_local(&v6)
                || is_ipv6_unique_local(&v6)
        }
    }
}

/// Syntactic host checks that don't require DNS: a literal IP in a
/// disallowed range, `.internal`, `localhost`, or the host's own domain.
fn is_disallowed_literal_host(host: &str, own_domain: &str, allow_loopback: bool) -> bool {
    let host_lc = host.trim_end_matches('.').to_ascii_lowercase();
    if let Ok(ip) = host_lc.parse::<IpAddr>() {
        return is_disallowed_ip(ip, allow_loopback);
    }
    if !allow_loopback && (host_lc == "localhost" || host_lc.ends_with(".localhost")) {
        return true;
    }
    if host_lc == "internal" || host_lc.ends_with(".internal") {
        return true;
    }
    let own_lc = own_domain.trim_end_matches('.').to_ascii_lowercase();
    if !own_lc.is_empty() && (host_lc == own_lc || host_lc.ends_with(&format!(".{own_lc}"))) {
        return true;
    }
    false
}

// ---- DNS-vetting resolver (requirement 8 / AC9) ----------------------------

/// Iteration 1 (clippy `type_complexity`): named per the lint's own
/// suggestion rather than inlined at each use.
pub type LookupFuture = Pin<Box<dyn Future<Output = std::io::Result<Vec<IpAddr>>> + Send>>;

/// Resolves a hostname to addresses. The production implementation
/// ([`HickoryLookup`]) is backed by `hickory-resolver`; tests substitute a
/// fixed-answer fake so a DNS name resolving to a private address (AC9) is
/// testable without depending on real network DNS.
pub trait NameLookup: Send + Sync {
    fn lookup(&self, host: String) -> LookupFuture;
}

/// Production [`NameLookup`]: a real `hickory-resolver` client, queried for
/// both A and AAAA records.
pub struct HickoryLookup(hickory_resolver::TokioResolver);

impl HickoryLookup {
    pub fn new() -> Result<Self, KindError> {
        let mut builder = hickory_resolver::Resolver::builder_tokio()
            .map_err(|e| KindError::Exec(format!("dns resolver init: {e}")))?;
        builder.options_mut().ip_strategy = hickory_resolver::config::LookupIpStrategy::Ipv4AndIpv6;
        let resolver = builder
            .build()
            .map_err(|e| KindError::Exec(format!("dns resolver build: {e}")))?;
        Ok(Self(resolver))
    }
}

impl NameLookup for HickoryLookup {
    fn lookup(&self, host: String) -> LookupFuture {
        let resolver = self.0.clone();
        Box::pin(async move {
            let lookup = resolver
                .lookup_ip(host)
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            Ok(lookup.iter().collect())
        })
    }
}

/// The marker error [`VettingResolver::resolve`] raises to refuse handing
/// `reqwest` an address to connect to; [`classify_reqwest_error`] downcasts
/// the error chain to find it so the refusal is reported as
/// `host_not_allowed`, not a generic `upstream_unreachable`.
#[derive(Debug, Clone)]
struct HostNotAllowed(String);

impl std::fmt::Display for HostNotAllowed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "host not allowed: {}", self.0)
    }
}

impl std::error::Error for HostNotAllowed {}

/// The single source of truth for "may `reqwest` connect here": every DNS
/// name `reqwest` needs an address for goes through this before connect,
/// vetting every resolved address against the same rules `validate` uses
/// syntactically at publish time. Reused as the actual `reqwest::dns::Resolve`
/// the client is built with (not a side-channel pre-check), which is what
/// closes the rebinding window -- the address vetted here is the address
/// `reqwest` connects to.
struct VettingResolver {
    own_domain: String,
    allow_loopback: bool,
    lookup: Arc<dyn NameLookup>,
}

impl Resolve for VettingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let own_domain = self.own_domain.clone();
        let allow_loopback = self.allow_loopback;
        let lookup = self.lookup.clone();
        Box::pin(async move {
            let host = name.as_str().to_string();
            if is_disallowed_literal_host(&host, &own_domain, allow_loopback) {
                return Err(
                    Box::new(HostNotAllowed(host)) as Box<dyn std::error::Error + Send + Sync>
                );
            }
            let ips = lookup
                .lookup(host.clone())
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            for ip in &ips {
                if is_disallowed_ip(*ip, allow_loopback) {
                    return Err(
                        Box::new(HostNotAllowed(host)) as Box<dyn std::error::Error + Send + Sync>
                    );
                }
            }
            let addrs: Addrs = Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// Walks a `reqwest::Error`'s source chain for the [`HostNotAllowed`]
/// marker our resolver raises; falls back to timeout/generic-unreachable
/// classification when it isn't there.
fn classify_reqwest_error(err: reqwest::Error) -> KindError {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&err);
    while let Some(e) = source {
        if let Some(hna) = e.downcast_ref::<HostNotAllowed>() {
            return KindError::structured("host_not_allowed", format!("{hna}"));
        }
        source = e.source();
    }
    if err.is_timeout() {
        KindError::structured(
            "upstream_timeout",
            format!("upstream request timed out: {err}"),
        )
    } else {
        KindError::structured(
            "upstream_unreachable",
            format!("upstream unreachable: {err}"),
        )
    }
}

// ---- rate limiting (P1 requirement 10 / AC13) ------------------------------

/// A per-tenant sliding window over the trailing 60s, shared by every
/// `http` tool a tenant has published (this crate registers one [`HttpKind`]
/// instance for the whole process, so its limiter is naturally shared
/// across tools -- the PRD's "across `http` tools" scope).
struct RateLimiter {
    limit: u32,
    per_tenant: Mutex<HashMap<i64, Vec<Instant>>>,
}

impl RateLimiter {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            per_tenant: Mutex::new(HashMap::new()),
        }
    }

    /// `Ok(())` and records the call, or `Err(retry_after_s)` when the
    /// tenant is already at the limit within the trailing window. The
    /// PRD's open question ("does `host.tool_test` count against the rate
    /// limit?") is resolved here as yes -- a test call reaches the real
    /// upstream, so it consumes the same outbound quota as an ordinary
    /// call.
    fn check(&self, tenant_id: i64) -> Result<(), u64> {
        let now = Instant::now();
        let mut guard = self
            .per_tenant
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = guard.entry(tenant_id).or_default();
        entries.retain(|t| now.saturating_duration_since(*t) < RATE_LIMIT_WINDOW);
        if entries.len() as u32 >= self.limit {
            let oldest = entries[0];
            let retry = RATE_LIMIT_WINDOW.saturating_sub(now.saturating_duration_since(oldest));
            return Err(retry.as_secs().max(1));
        }
        entries.push(now);
        Ok(())
    }
}

// ---- redaction (requirement 6, AC1 / AC12) --------------------------------

/// Replaces every occurrence of any resolved secret value with `***` in a
/// string that could leave the host (a result, a log line, an error body).
/// Longest-first so one secret value that happens to be a substring of
/// another doesn't leave a partial value visible.
fn redact_str(input: &str, secrets: &[String]) -> String {
    let mut ordered: Vec<&String> = secrets.iter().filter(|s| !s.is_empty()).collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = input.to_string();
    for secret in ordered {
        out = out.replace(secret.as_str(), "***");
    }
    out
}

fn redact_value(value: &Value, secrets: &[String]) -> Value {
    if secrets.is_empty() {
        return value.clone();
    }
    match value {
        Value::String(s) => Value::String(redact_str(s, secrets)),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), redact_value(v, secrets)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.iter().map(|v| redact_value(v, secrets)).collect()),
        other => other.clone(),
    }
}

// ---- templating (requirement 2) --------------------------------------------

/// `minijinja::Environment::empty()` (no built-in filters, no auto-escape)
/// plus exactly `urlencode`/`tojson`/`default` re-added, in `Strict`
/// undefined mode.
fn build_template_env() -> minijinja::Environment<'static> {
    let mut env = minijinja::Environment::empty();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.add_filter("urlencode", minijinja::filters::urlencode);
    env.add_filter("tojson", minijinja::filters::tojson);
    env.add_filter("default", minijinja::filters::default);
    env
}

fn render_str(
    env: &minijinja::Environment<'_>,
    field: &str,
    template: &str,
    context: &Value,
) -> Result<String, KindError> {
    env.render_str(template, context).map_err(|e| {
        KindError::structured_with(
            "template_error",
            format!("{field}: {e}"),
            json!({"field": field}),
        )
    })
}

/// Renders every string leaf of a (possibly nested) JSON body template,
/// keeping its shape.
fn render_body(
    env: &minijinja::Environment<'_>,
    context: &Value,
    value: &Value,
    path: &str,
) -> Result<Value, KindError> {
    match value {
        Value::String(s) => Ok(Value::String(render_str(env, path, s, context)?)),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(
                    k.clone(),
                    render_body(env, context, v, &format!("{path}.{k}"))?,
                );
            }
            Ok(Value::Object(out))
        }
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for (i, v) in arr.iter().enumerate() {
                out.push(render_body(env, context, v, &format!("{path}[{i}]"))?);
            }
            Ok(Value::Array(out))
        }
        other => Ok(other.clone()),
    }
}

fn header_map_to_json(headers: &reqwest::header::HeaderMap) -> Value {
    let mut map = Map::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            map.insert(name.as_str().to_string(), Value::String(v.to_string()));
        }
    }
    Value::Object(map)
}

// ---- the kind ---------------------------------------------------------------

pub struct HttpKind {
    own_domain: String,
    allow_loopback: bool,
    /// Test-only, alongside `allow_loopback`: `wiremock` (this project's
    /// stub-upstream crate) speaks plain HTTP, not HTTPS, so the functional
    /// tests that need a reachable stub (redirects, 429, timeouts, body
    /// caps, ...) run against a kind built with this set. The SSRF-specific
    /// tests (AC2, AC9) instead use [`HttpKind::for_test_strict`], which
    /// keeps the production https-only / loopback-blocked policy -- neither
    /// needs a reachable upstream, since both are rejected before any
    /// connection is attempted.
    allow_http: bool,
    client: reqwest::Client,
    limiter: RateLimiter,
}

impl HttpKind {
    /// Production constructor: DNS resolution is real (`hickory-resolver`),
    /// only `https://` to a public host is ever attempted, and the rate
    /// limit is the PRD's 600/min.
    pub fn new(own_domain: impl Into<String>) -> Result<Self, KindError> {
        let lookup: Arc<dyn NameLookup> = Arc::new(HickoryLookup::new()?);
        Self::build(
            own_domain.into(),
            lookup,
            false,
            false,
            DEFAULT_RATE_LIMIT_PER_MINUTE,
        )
    }

    /// Test constructor for the functional (non-SSRF) test suite: `lookup`
    /// stands in for real DNS, loopback and plain `http://` are both
    /// allowed so a test can call a local `wiremock` stub upstream while
    /// exercising the *real* template / redaction / rate-limit / error-code
    /// paths, not a bypassed version of them.
    pub fn for_test(own_domain: impl Into<String>, lookup: Arc<dyn NameLookup>) -> Self {
        Self::build(
            own_domain.into(),
            lookup,
            true,
            true,
            DEFAULT_RATE_LIMIT_PER_MINUTE,
        )
        .expect("building a reqwest client for tests must not fail") // allowlist: test-only expect
    }

    pub fn for_test_with_rate_limit(
        own_domain: impl Into<String>,
        lookup: Arc<dyn NameLookup>,
        limit_per_minute: u32,
    ) -> Self {
        Self::build(own_domain.into(), lookup, true, true, limit_per_minute)
            .expect("building a reqwest client for tests must not fail") // allowlist: test-only expect
    }

    /// Test constructor with the *production* SSRF policy (https-only,
    /// loopback blocked) but an injectable [`NameLookup`], for AC2/AC9:
    /// both reject before any connection is attempted, so neither needs a
    /// reachable upstream.
    pub fn for_test_strict(own_domain: impl Into<String>, lookup: Arc<dyn NameLookup>) -> Self {
        Self::build(
            own_domain.into(),
            lookup,
            false,
            false,
            DEFAULT_RATE_LIMIT_PER_MINUTE,
        )
        .expect("building a reqwest client for tests must not fail") // allowlist: test-only expect
    }

    fn build(
        own_domain: String,
        lookup: Arc<dyn NameLookup>,
        allow_loopback: bool,
        allow_http: bool,
        rate_limit_per_minute: u32,
    ) -> Result<Self, KindError> {
        let resolver = VettingResolver {
            own_domain: own_domain.clone(),
            allow_loopback,
            lookup,
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(resolver))
            .build()
            .map_err(|e| KindError::Exec(format!("building the http kind's client: {e}")))?;
        Ok(Self {
            own_domain,
            allow_loopback,
            allow_http,
            client,
            limiter: RateLimiter::new(rate_limit_per_minute),
        })
    }

    fn scheme_allowed(&self, scheme: &str) -> bool {
        scheme == "https" || (self.allow_http && scheme == "http")
    }
}

#[async_trait::async_trait]
impl Kind for HttpKind {
    fn name(&self) -> &'static str {
        "http"
    }

    fn validate(&self, spec: &Value) -> Result<(), KindError> {
        let parsed = parse_spec(spec)?;

        let method = parsed.method.to_ascii_uppercase();
        if !ALLOWED_METHODS.contains(&method.as_str()) {
            return Err(KindError::InvalidSpec(format!(
                "method: must be one of {}; got '{}'",
                ALLOWED_METHODS.join(", "),
                parsed.method
            )));
        }

        if let Some(t) = parsed.timeout_s
            && !(1..=MAX_TIMEOUT_S).contains(&t)
        {
            return Err(KindError::InvalidSpec(format!(
                "timeout_s: must be between 1 and {MAX_TIMEOUT_S}; got {t}"
            )));
        }

        if let Some(r) = &parsed.response
            && r != "json"
            && r != "text"
        {
            return Err(KindError::InvalidSpec(format!(
                "response: must be 'json' or 'text'; got '{r}'"
            )));
        }

        if parsed.body.is_some() && matches!(method.as_str(), "GET" | "DELETE") {
            return Err(KindError::InvalidSpec(
                "body: not allowed for GET/DELETE".into(),
            ));
        }

        let (scheme, host) = split_scheme_host(&parsed.url)
            .ok_or_else(|| KindError::InvalidSpec("url: could not parse scheme/host".into()))?;
        if host.is_empty() {
            return Err(KindError::InvalidSpec("url: missing host".into()));
        }
        if !self.scheme_allowed(scheme) {
            return Err(KindError::structured_with(
                "host_not_allowed",
                "url: must use https://",
                json!({"field": "url"}),
            ));
        }
        if is_disallowed_literal_host(host, &self.own_domain, self.allow_loopback) {
            return Err(KindError::structured_with(
                "host_not_allowed",
                format!("url: host '{host}' is not a publicly callable host"),
                json!({"field": "url"}),
            ));
        }

        for key in parsed.headers.keys() {
            reqwest::header::HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
                KindError::InvalidSpec(format!("headers.{key}: not a valid header name: {e}"))
            })?;
        }

        // Requirement 1: an author-supplied schema is checked exactly as
        // before; an absent one is derived right here (requirement 4) --
        // unlike `python`, template parsing has no untrusted-execution
        // concern, so there is no async/sandbox boundary to defer to (see
        // `infer` module docs).
        match &parsed.args_schema {
            Some(schema) => {
                jsonschema::validator_for(schema).map_err(|e| {
                    KindError::InvalidSpec(format!("args_schema: not a valid JSON Schema: {e}"))
                })?;
            }
            None => {
                parsed.effective_args_schema()?;
            }
        }

        Ok(())
    }

    fn describe(&self, spec: &Value) -> ToolDescriptor {
        match parse_spec(spec) {
            Ok(parsed) => {
                let description = parsed.description.clone().unwrap_or_else(|| {
                    format!(
                        "Calls {} {} and returns the response.",
                        parsed.method.to_ascii_uppercase(),
                        parsed.url
                    )
                });
                // Requirement 9 (shared with `python`): a source whose
                // inference would fail can't have been published (`validate`
                // gated it above), so this fallback is unreachable in
                // practice; it exists so `describe` never panics.
                let input_schema = parsed
                    .effective_args_schema()
                    .unwrap_or_else(|_| json!({"type": "object"}));
                ToolDescriptor {
                    name: "http".to_string(),
                    description,
                    input_schema,
                }
            }
            Err(e) => ToolDescriptor {
                name: "http".to_string(),
                description: format!("invalid http tool spec: {e}"),
                input_schema: json!({"type": "object"}),
            },
        }
    }

    fn referenced_secrets(&self, spec: &Value) -> Vec<String> {
        parse_spec(spec)
            .map(|parsed| referenced_secrets_in_spec(&parsed).into_iter().collect())
            .unwrap_or_default()
    }

    async fn call(&self, spec: &Value, args: Value, ctx: &CallCtx) -> Result<Value, KindError> {
        let parsed = parse_spec(spec)?;
        let method_str = parsed.method.to_ascii_uppercase();
        let method = reqwest::Method::from_bytes(method_str.as_bytes())
            .map_err(|e| KindError::InvalidSpec(format!("method: {e}")))?;

        // Defensive re-validation (the host's generic dispatch already
        // checks `args` against `describe()`'s schema before `call` is
        // invoked, and so does `host.tool_test`/the conformance suite) --
        // but AC4 pins this kind's own code to `args_invalid` with the
        // schema path, distinct from the host's generic `args_invalid`
        // pre-check message.
        let effective_schema = parsed.effective_args_schema()?;
        let validator = jsonschema::validator_for(&effective_schema)
            .map_err(|e| KindError::InvalidSpec(format!("args_schema: {e}")))?;
        if let Err(e) = validator.validate(&args) {
            let data = json!({
                "schema_path": e.schema_path.to_string(),
                "instance_path": e.instance_path.to_string(),
            });
            return Err(KindError::structured_with(
                "args_invalid",
                e.to_string(),
                data,
            ));
        }

        if let Err(retry_after_s) = self.limiter.check(ctx.tenant_id) {
            return Err(KindError::structured_with(
                "rate_limited",
                "outbound http-tool rate limit exceeded (600 calls/min)",
                json!({"retry_after_s": retry_after_s}),
            ));
        }

        let args_obj = args.as_object().cloned().unwrap_or_default();
        let referenced = referenced_secrets_in_spec(&parsed);
        let mut secret_map = Map::new();
        let mut secret_values: Vec<String> = Vec::new();
        for name in &referenced {
            if let Some(value) = ctx.secrets.resolve(name) {
                secret_values.push(value.clone());
                secret_map.insert(name.clone(), Value::String(value));
            }
        }

        let mut known: BTreeSet<String> = args_obj.keys().cloned().collect();
        for name in secret_map.keys() {
            known.insert(format!("secret.{name}"));
        }

        let mut context_obj = args_obj;
        context_obj.insert("secret".to_string(), Value::Object(secret_map));
        let context = Value::Object(context_obj);

        // Pre-check every template for a reference this call can't satisfy
        // -- gives `template_error` the exact variable name (AC8) without
        // depending on minijinja's own error-detail format.
        for (field, template) in template_fields(&parsed) {
            if let Some(bad) = find_undefined_ref(&template, &known) {
                return Err(KindError::structured_with(
                    "template_error",
                    format!("{field}: undefined variable '{bad}'"),
                    json!({"field": field, "variable": bad}),
                ));
            }
        }

        let env = build_template_env();
        let rendered_url = render_str(&env, "url", &parsed.url, &context)?;
        let (scheme, host) = split_scheme_host(&rendered_url).ok_or_else(|| {
            KindError::structured_with(
                "host_not_allowed",
                "url: did not render to a valid https URL",
                json!({"field": "url"}),
            )
        })?;
        if !self.scheme_allowed(scheme) {
            return Err(KindError::structured_with(
                "host_not_allowed",
                "url: rendered url must use https://",
                json!({"field": "url"}),
            ));
        }
        // A literal IP in the rendered host never reaches our DNS resolver
        // hook (reqwest skips resolution for an address it already has),
        // so it must be checked here explicitly; an actual hostname is
        // checked by `VettingResolver` right before `reqwest` connects.
        if let Ok(ip) = host.parse::<IpAddr>()
            && is_disallowed_ip(ip, self.allow_loopback)
        {
            return Err(KindError::structured_with(
                "host_not_allowed",
                format!("url: rendered host '{host}' is not a publicly callable address"),
                json!({"field": "url"}),
            ));
        }

        let mut header_map = reqwest::header::HeaderMap::new();
        for (key, template) in &parsed.headers {
            let field = format!("headers.{key}");
            let value = render_str(&env, &field, template, &context)?;
            let header_name =
                reqwest::header::HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
                    KindError::structured_with(
                        "template_error",
                        format!("{field}: not a valid header name: {e}"),
                        json!({"field": field}),
                    )
                })?;
            let header_value = reqwest::header::HeaderValue::from_str(&value).map_err(|e| {
                KindError::structured_with(
                    "template_error",
                    format!("{field}: rendered value is not a valid header value: {e}"),
                    json!({"field": field}),
                )
            })?;
            header_map.insert(header_name, header_value);
        }

        let mut query_pairs: Vec<(String, String)> = Vec::with_capacity(parsed.query.len());
        for (key, template) in &parsed.query {
            let field = format!("query.{key}");
            query_pairs.push((key.clone(), render_str(&env, &field, template, &context)?));
        }

        let rendered_body = parsed
            .body
            .as_ref()
            .map(|b| render_body(&env, &context, b, "body"))
            .transpose()?;

        let request_summary = if ctx.test_mode {
            let query_json: Map<String, Value> = query_pairs
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            Some(redact_value(
                &json!({
                    "method": method_str,
                    "url": rendered_url,
                    "headers": header_map_to_json(&header_map),
                    "query": query_json,
                    "body": rendered_body,
                }),
                &secret_values,
            ))
        } else {
            None
        };

        let mut builder = self
            .client
            .request(method, rendered_url.as_str())
            .timeout(Duration::from_secs(parsed.effective_timeout_s()))
            .headers(header_map)
            .query(&query_pairs);
        builder = match &rendered_body {
            Some(Value::String(s)) => builder.body(s.clone()),
            Some(other) => builder.json(other),
            None => builder,
        };

        let start = Instant::now();
        let response = match builder.send().await {
            Ok(r) => r,
            Err(e) => return Err(classify_reqwest_error(e)),
        };

        let status = response.status();
        let response_headers = response.headers().clone();
        let retry_after_s = response_headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok());

        let mut buf: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        let mut too_large = false;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => return Err(classify_reqwest_error(e)),
            };
            if buf.len() + chunk.len() > crate::state::MAX_CALL_RESULT_BYTES {
                too_large = true;
                break;
            }
            buf.extend_from_slice(&chunk);
        }
        drop(stream);
        let duration_ms = start.elapsed().as_millis();

        if too_large {
            ctx.log.log(&format!(
                "{method_str} {host} response_too_large {duration_ms}ms"
            ));
            return Err(KindError::structured(
                "response_too_large",
                format!(
                    "upstream response exceeded the {}-byte cap",
                    crate::state::MAX_CALL_RESULT_BYTES
                ),
            ));
        }

        ctx.log.log(&format!(
            "{method_str} {host} {} {duration_ms}ms",
            status.as_u16()
        ));

        if status.is_client_error() || status.is_server_error() {
            let excerpt_len = buf.len().min(512);
            let excerpt = redact_str(
                &String::from_utf8_lossy(&buf[..excerpt_len]),
                &secret_values,
            );
            let mut data = json!({"upstream_status": status.as_u16(), "body_excerpt": excerpt});
            if let Some(retry) = retry_after_s {
                data["retry_after_s"] = json!(retry);
            }
            return Err(KindError::structured_with(
                "upstream_status",
                format!("upstream returned HTTP {}", status.as_u16()),
                data,
            ));
        }

        let content_type = response_headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let body_value = if parsed.effective_response() == "json"
            && content_type.to_ascii_lowercase().contains("json")
        {
            serde_json::from_slice(&buf)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&buf).to_string()))
        } else {
            Value::String(String::from_utf8_lossy(&buf).to_string())
        };

        let mut headers_out = Map::new();
        for name in RESPONSE_HEADER_SUBSET {
            if let Some(v) = response_headers.get(name).and_then(|v| v.to_str().ok()) {
                headers_out.insert(name.to_string(), Value::String(v.to_string()));
            }
        }

        let result = redact_value(
            &json!({
                "status": status.as_u16(),
                "headers": Value::Object(headers_out),
                "body": body_value,
            }),
            &secret_values,
        );

        if ctx.test_mode {
            // Requirement 10 / AC15: `host.tool_test` shows the schema the
            // host inferred (or the author's own, unchanged) alongside the
            // rendered request/response it already echoed, so a tenant can
            // inspect it before relying on it.
            Ok(json!({"request": request_summary, "response": result, "schema": effective_schema}))
        } else {
            Ok(result)
        }
    }

    fn example(&self) -> KindExample {
        KindExample {
            spec: json!({
                "method": "GET",
                "url": "https://api.example.com/items/{{id}}",
            }),
            call_args: json!({"id": "123"}),
            blurb: "url must be an absolute https URL; method and url are the only \
                required fields -- args_schema is inferred from the url/header/body \
                templates when omitted.",
        }
    }
}
