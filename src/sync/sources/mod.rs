//! PRD-mcphost-sync-pull-with-change-detection R6: one file per
//! [`SourceKind`](super::SourceKind), each exposing `fetch(ctx, cursor)`;
//! [`super::pull::fetch`] is the exhaustive dispatch. `api_base_override`
//! lets a test point an OAuth kind at a fixture server.

pub mod airtable;
pub mod csv_url;
pub mod google_sheet;
pub mod http_json;
pub mod notion_db;

use super::SourceKind;
use crate::state::AppState;

#[cfg(feature = "test-support")]
static API_BASES: std::sync::Mutex<Vec<(usize, &'static str, String)>> = std::sync::Mutex::new(Vec::new());

/// Test support: `kind` pulls from `base` instead of its provider for syncs
/// run through `state`.
#[cfg(feature = "test-support")]
pub fn set_api_base_for_test(state: &AppState, kind: SourceKind, base: &str) {
    let mut guard = API_BASES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.retain(|(s, k, _)| !(*s == state as *const AppState as usize && *k == kind.as_str()));
    guard.push((state as *const AppState as usize, kind.as_str(), base.trim_end_matches('/').to_string()));
}

/// The API base a fixture set for `kind` on this `state`, if any.
#[cfg(feature = "test-support")]
pub(crate) fn api_base_override(state: &AppState, kind: SourceKind) -> Option<String> {
    let guard = API_BASES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .iter()
        .find(|(s, k, _)| *s == state as *const AppState as usize && *k == kind.as_str())
        .map(|(_, _, b)| b.clone())
}

#[cfg(not(feature = "test-support"))]
pub(crate) fn api_base_override(_state: &AppState, _kind: SourceKind) -> Option<String> {
    None
}
