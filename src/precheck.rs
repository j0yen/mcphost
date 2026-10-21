//! PRD-mcphost-implicit-signup-mints-only-on-success R1: the one module that
//! checks a call before it dispatches. [`precheck_tenant_call`] (the `fields`
//! check, the argument validation and the name-exists check) and
//! [`precheck_args`] (the argument validation alone) are the only entry points;
//! the checks they compose (`host_args::validate_host_args`,
//! `paged::refuse_fields_on_unprojected`) are `pub(crate)` and called from here
//! only (`imos_ac02` scans `src/` for any other caller), so the pre-mint check
//! of the implicit signup and the dispatch check cannot drift.
//!
//! ```compile_fail,E0603
//! let kinds = mcphost::kinds::KindRegistry::default();
//! let _ = mcphost::host_args::validate_host_args(&kinds, "host.whoami", &serde_json::json!({}));
//! ```
//!
//! ```compile_fail,E0603
//! let _ = mcphost::paged::refuse_fields_on_unprojected("host.whoami", &serde_json::json!({}));
//! ```

use crate::errors::AppError;
use crate::kinds::KindRegistry;
use serde_json::Value;

/// Validates `args` against `name`'s descriptor only, for callers that have no
/// tenant tool to look up (admin dispatch, the pre-dispatch path, preauth).
pub fn precheck_args(kinds: &KindRegistry, name: &str, args: &Value) -> Result<(), AppError> {
    crate::host_args::validate_host_args(kinds, name, args)
}

/// The full pre-dispatch check of a tenant call: `fields` on an unprojected
/// verb, the descriptor validation, then that `name` is served at all (a
/// dispatch-table verb, an unadvertised arm, or `host.msg.wait`). An unknown
/// name is the `unknown_host_tool` error dispatch itself returns.
pub fn precheck_tenant_call(kinds: &KindRegistry, name: &str, args: &Value) -> Result<(), AppError> {
    crate::paged::refuse_fields_on_unprojected(name, args)?;
    crate::host_args::validate_host_args(kinds, name, args)?;
    if crate::dispatch_table::lookup(name).is_some()
        || crate::handler::UNADVERTISED_ARMS.contains(&name)
        || name == "host.msg.wait"
    {
        Ok(())
    } else {
        Err(crate::handler::unknown_host_tool(kinds, name))
    }
}
