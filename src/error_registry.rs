//! PRD-mcphost-errors-json-contract: the one table of every error code the
//! host can return. `GET /errors.json`, `host.errors` and the `class` /
//! `family` / `remedy` / `help_url` fields `into_error_data` puts in every
//! error's `data` are all generated from [`ERROR_CODES`]; the
//! `errjson_ac01` scan test fails when a code appears in `src/` that is not
//! a row here, so the table cannot drift from the code.

use std::sync::LazyLock;

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::doc_anchors::DocAnchor;
use crate::errors::AppError;

/// Whose fault an error is (the five-name set `/errors.json` lists under
/// `classes`, generated from these variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// The caller sent something wrong; fix the call.
    AgentFault,
    /// The host declined by policy or state; the call was understood.
    HostRefusal,
    /// An upstream the tool depends on failed.
    UpstreamFault,
    /// A plan ceiling or rate was hit.
    PlanLimit,
    /// The host itself failed.
    HostFault,
}

impl ErrorClass {
    pub const ALL: &'static [ErrorClass] = &[
        ErrorClass::AgentFault,
        ErrorClass::HostRefusal,
        ErrorClass::UpstreamFault,
        ErrorClass::PlanLimit,
        ErrorClass::HostFault,
    ];

    /// The wire name; the exhaustive match keeps [`Self::ALL`] honest.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorClass::AgentFault => "agent_fault",
            ErrorClass::HostRefusal => "host_refusal",
            ErrorClass::UpstreamFault => "upstream_fault",
            ErrorClass::PlanLimit => "plan_limit",
            ErrorClass::HostFault => "host_fault",
        }
    }
}

impl ErrorClass {
    /// One sentence for `/help/<code>` pages generated from the registry.
    pub const fn describe(self) -> &'static str {
        match self {
            ErrorClass::AgentFault => "The call itself was wrong; changing the call fixes it.",
            ErrorClass::HostRefusal => "The host understood the call and declined it by policy or state.",
            ErrorClass::UpstreamFault => "A service this tool depends on failed or refused the request.",
            ErrorClass::PlanLimit => "A plan ceiling or rate limit was reached.",
            ErrorClass::HostFault => "The host itself failed; it is not the caller's mistake.",
        }
    }
}

/// Which part of the product a code belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Auth,
    Args,
    Spec,
    Publish,
    Call,
    Upstream,
    Plan,
    Compose,
    Storage,
    Internal,
    Oauth,
    Share,
    Registry,
}

impl Family {
    pub const fn as_str(self) -> &'static str {
        match self {
            Family::Auth => "auth",
            Family::Args => "args",
            Family::Spec => "spec",
            Family::Publish => "publish",
            Family::Call => "call",
            Family::Upstream => "upstream",
            Family::Plan => "plan",
            Family::Compose => "compose",
            Family::Storage => "storage",
            Family::Internal => "internal",
            Family::Oauth => "oauth",
            Family::Share => "share",
            Family::Registry => "registry",
        }
    }

    /// The doc section a family's codes are explained under (a
    /// compile-checked [`DocAnchor`], so it cannot name a missing heading).
    const fn docs_anchor(self) -> DocAnchor {
        match self {
            Family::Auth | Family::Oauth => DocAnchor::DocsAgentQuickstartQuickstartForAgents,
            Family::Args | Family::Publish | Family::Share | Family::Registry => {
                DocAnchor::DocsToolsTools
            }
            Family::Spec | Family::Compose => DocAnchor::ReadmeKinds,
            Family::Call => DocAnchor::ReadmeCallLimits,
            Family::Upstream => DocAnchor::ReadmeEgressProxyNetworkPublicEgress,
            Family::Plan => DocAnchor::DocsPlansPlanCeilings,
            Family::Storage | Family::Internal => DocAnchor::ReadmeGettingHelp,
        }
    }
}

/// One row of [`ERROR_CODES`].
#[derive(Debug, Clone, Copy)]
pub struct ErrorEntry {
    pub code: &'static str,
    pub family: Family,
    pub class: ErrorClass,
    /// Imperative, at most 200 chars; names the `data` field to read when one exists.
    pub remedy: &'static str,
    pub docs_anchor: DocAnchor,
    pub retryable: bool,
    /// `file:line` of the `err_code!` declaration (or hand row); never serialized.
    pub site: &'static str,
}

impl ErrorEntry {
    /// The seven-field JSON row (`help_url` is built on `base`).
    pub fn json_row(&self, base: &str) -> Value {
        json!({
            "code": self.code,
            "family": self.family.as_str(),
            "class": self.class.as_str(),
            "remedy": self.remedy,
            "docs_anchor": self.docs_anchor.as_str(),
            "retryable": self.retryable,
            "help_url": format!("{}/help/{}", base.trim_end_matches('/'), self.code),
        })
    }
}

/// The one constructor of an [`ErrorEntry`]; [`err_code!`] and the legacy
/// hand table both build rows through it.
#[doc(hidden)]
pub const fn row(
    code: &'static str,
    family: Family,
    class: ErrorClass,
    retryable: bool,
    remedy: &'static str,
) -> ErrorEntry {
    ErrorEntry { code, family, class, remedy, docs_anchor: family.docs_anchor(), retryable, site: "src/error_registry.rs" }
}

#[doc(hidden)]
pub use linkme;

/// Every `err_code!` declaration in the crate, filled by the linker.
#[linkme::distributed_slice]
pub static REGISTERED: [ErrorEntry];

/// Declare an error code where it is raised: defines the `&'static str`
/// const the raise site uses and registers its [`ErrorEntry`] into
/// [`REGISTERED`]. Item form only, so a code raised at two sites is declared
/// once and referenced by path.
///
/// ```ignore
/// err_code! {
///     pub RESOURCES_MAX,
///     code: "resources_max", family: Plan, class: PlanLimit, retryable: false,
///     remedy: "Remove a resource or move to a plan with a higher ceiling.",
/// }
/// ```
#[macro_export]
macro_rules! err_code {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident,
        code: $code:literal, family: $family:ident, class: $class:ident,
        retryable: $retryable:literal, remedy: $remedy:expr $(,)?
    ) => {
        $(#[$meta])*
        $vis const $name: &'static str = $code;
        const _: () = {
            #[$crate::error_registry::linkme::distributed_slice($crate::error_registry::REGISTERED)]
            #[linkme(crate = $crate::error_registry::linkme)]
            static ENTRY: $crate::error_registry::ErrorEntry = $crate::error_registry::ErrorEntry {
                site: concat!(file!(), ":", line!()),
                ..$crate::error_registry::row(
                    $code,
                    $crate::error_registry::Family::$family,
                    $crate::error_registry::ErrorClass::$class,
                    $retryable,
                    $remedy,
                )
            };
        };
    };
    // Arm form for the `AppError::code()` block in errors.rs, where the
    // declaration reads like the `=> "code"` arm it replaces.
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident => $code:literal,
        $family:ident, $class:ident, $retryable:literal, $remedy:expr $(,)?
    ) => {
        $crate::err_code! {
            $(#[$meta])* $vis $name,
            code: $code, family: $family, class: $class,
            retryable: $retryable, remedy: $remedy,
        }
    };
}

/// The registry row for `code`, if any.
pub fn lookup(code: &str) -> Option<&'static ErrorEntry> {
    let sorted = sorted_entries();
    sorted.binary_search_by(|e| e.code.cmp(code)).ok().map(|i| &sorted[i])
}

fn sorted_entries() -> &'static [ErrorEntry] {
    static SORTED: LazyLock<Vec<ErrorEntry>> = LazyLock::new(|| {
        let mut all: Vec<ErrorEntry> = REGISTERED.iter().copied().collect();
        all.sort_by_key(|e| e.code);
        all
    });
    &SORTED
}

/// Every code the host can return, sorted by code: a lazily sorted view
/// over [`REGISTERED`] (the `err_code!` declarations).
#[derive(Debug, Clone, Copy)]
pub struct ErrorCodes;

pub const ERROR_CODES: ErrorCodes = ErrorCodes;

impl ErrorCodes {
    pub fn iter(&self) -> std::slice::Iter<'static, ErrorEntry> {
        sorted_entries().iter()
    }
    pub fn len(&self) -> usize {
        sorted_entries().len()
    }
    pub fn is_empty(&self) -> bool {
        sorted_entries().is_empty()
    }
}

impl IntoIterator for ErrorCodes {
    type Item = &'static ErrorEntry;
    type IntoIter = std::slice::Iter<'static, ErrorEntry>;
    fn into_iter(self) -> Self::IntoIter {
        sorted_entries().iter()
    }
}

/// The `/errors.json` / `host.errors` object, built from [`ERROR_CODES`]
/// (sorted by code) on `base`, this request's own origin.
pub fn catalog_json(base: &str) -> Value {
    let mut rows: Vec<&ErrorEntry> = ERROR_CODES.iter().collect();
    rows.sort_by_key(|e| e.code);
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "classes": ErrorClass::ALL.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
        "codes": rows.iter().map(|e| e.json_row(base)).collect::<Vec<_>>(),
    })
}

/// `host.errors`'s description (it is the tool-list text an agent reads).
pub const TOOL_DESCRIPTION: &str = "List every error code this host can return, each with its class \
    (agent_fault | host_refusal | upstream_fault | plan_limit | host_fault), family, remedy, docs \
    anchor, retryable flag and help_url -- the same object GET /errors.json serves. Pass code to get \
    one row; a misspelt code answers tool_not_found with did_you_mean. Read-only, anonymous, no \
    tenant data.";

/// `host.errors`'s input properties (`host_schema` adds `tenant_key`).
pub fn schema_properties() -> Value {
    json!({
        "code": {
            "type": "string",
            "description": "An error code, e.g. tool_exception. Omit to list every code.",
        },
    })
}

/// The codes nearest `requested` by edit distance (at most three, within
/// two edits), nearest first.
fn nearest_codes(requested: &str) -> Vec<&'static str> {
    let lower = requested.to_ascii_lowercase();
    let mut scored: Vec<(usize, &'static str)> = ERROR_CODES
        .iter()
        .map(|e| (crate::errors::levenshtein(&lower, e.code), e.code))
        .filter(|(d, _)| *d <= 2)
        .collect();
    scored.sort();
    scored.into_iter().take(3).map(|(_, c)| c).collect()
}

/// `host.errors {code?}`: with no `code` the [`catalog_json`] object (what
/// `/errors.json` serves); with one, that row. An unknown `code` is a
/// `tool_not_found`-shaped error naming the nearest code in `did_you_mean`.
pub fn call(base: &str, args: &Value) -> Result<Value, AppError> {
    let code = match args.get("code") {
        None | Some(Value::Null) => return Ok(catalog_json(base)),
        Some(Value::String(code)) => code,
        Some(_) => return Err(AppError::InvalidArgs("code: expected a string".to_string())),
    };
    if let Some(row) = lookup(code) {
        return Ok(row.json_row(base));
    }
    let nearest = nearest_codes(code);
    let mut data = Map::new();
    data.insert("error".to_string(), json!("unknown_error_code"));
    data.insert("name".to_string(), json!(code));
    data.insert("nearest".to_string(), json!(nearest));
    if let Some(best) = nearest.first() {
        data.insert("did_you_mean".to_string(), json!(best));
    }
    data.insert("hint".to_string(), json!("host.errors with no code lists every code"));
    Err(AppError::Structured {
        code: crate::errors::TOOL_NOT_FOUND,
        message: format!("unknown error code {code}; nearest: {}", nearest.join(", ")),
        data: Value::Object(data),
    })
}
