//! PRD-mcphost-response-size-budget-explicit-v2: the byte budget of a paged
//! verb's page and the one [`Truncated`] marker a cut page carries in
//! `_meta.mcphost.truncated {bytes, of, continue_with}`.
//!
//! The budget is the calling tenant's plan field `response_budget_bytes`
//! ([`budget_for`]), never a literal here. A page over it is cut at a row
//! boundary by [`cut_page`], the one helper the paged verbs share; the verb
//! leaves the marker on its value ([`note_truncated`]) and `handler::call_tool`
//! moves it into `_meta` ([`take_truncated`]), the way `paged::take_clamp`
//! hands over `limit_clamped`. Nothing else is cut: a non-paged response is
//! returned whole, or refused by the kinds' own wall (`tool_output_too_large`),
//! which is not this module's concern.
//!
//! `src/budget.rs` is the run-budget governor; this file is unrelated to it.

use serde_json::{Value, json};

use crate::paged::Cursor;
use crate::state::AppState;
use crate::db::Tenant;

/// Reserved key a paged verb leaves on its result when it cut the page for
/// size; `handler::call_tool` removes it and reports it as
/// `_meta.mcphost.truncated`.
pub const TRUNCATED_KEY: &str = "__truncated";

/// The call that reads the rows a cut page left behind: the request's own
/// arguments, `cursor` set to the last row kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub verb: String,
    pub args: Value,
}

/// `_meta.mcphost.truncated`: this page was cut for size.
#[derive(Debug, Clone, PartialEq)]
pub struct Truncated {
    /// Serialized size of the rows sent.
    pub bytes: u64,
    /// Serialized size of the rows fetched before the cut.
    pub of: u64,
    pub continue_with: Option<Call>,
}

impl Truncated {
    /// The wire shape: exactly `bytes`, `of` and `continue_with`.
    pub fn to_json(&self) -> Value {
        let continue_with = match &self.continue_with {
            Some(c) => json!({"verb": c.verb, "args": c.args}),
            None => Value::Null,
        };
        json!({"bytes": self.bytes, "of": self.of, "continue_with": continue_with})
    }
}

/// The page budget of `tenant`'s plan; an anonymous caller (no plan row)
/// reads the `free` plan's.
pub fn budget_for(state: &AppState, plan: &str) -> u64 {
    let catalog = &state.plans;
    let row = catalog.get(plan).or_else(|| catalog.get("free"));
    row.map(|p| p.response_budget_bytes.max(0) as u64).unwrap_or(0)
}

/// [`budget_for`] for a verb handler holding the calling tenant.
pub fn budget_of(state: &AppState, tenant: &Tenant) -> u64 {
    budget_for(state, &tenant.plan)
}

/// Serialized size of `rows` as a JSON array (what the client receives).
pub fn array_bytes(rows: &[Value]) -> u64 {
    if rows.is_empty() {
        return 2;
    }
    let body: usize = rows.iter().map(|r| serde_json::to_vec(r).map_or(0, |b| b.len())).sum();
    (body + (rows.len() - 1) + 2) as u64
}

/// How many leading rows of `rows` fit `budget` once serialized as a JSON
/// array, at least one when `rows` is not empty (a row larger than the budget
/// is sent alone).
pub fn rows_within(rows: &[Value], budget: u64) -> usize {
    let mut used: u64 = 2;
    for (i, row) in rows.iter().enumerate() {
        let size = serde_json::to_vec(row).map_or(0, |b| b.len() as u64) + u64::from(i > 0);
        if i > 0 && used + size > budget {
            return i;
        }
        used += size;
    }
    rows.len()
}

/// A page that [`cut_page`] shortened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    pub bytes: u64,
    pub of: u64,
}

/// Cut a page at a row boundary when its serialized rows pass `budget`.
///
/// `rows` are the page's domain rows and `json` their wire form, the same
/// length and order (projection already applied to `json`, so the size is what
/// the client receives). Trailing rows are dropped from both until the array
/// fits, at least one row staying; `next` becomes the cursor of the last row
/// kept, so the dropped rows are the next page and nothing overlaps or is
/// skipped. Returns `None`, touching nothing, when the page fits.
pub fn cut_page<T>(
    rows: &mut Vec<T>,
    json: &mut Vec<Value>,
    next: &mut Option<Cursor>,
    budget: u64,
    cursor_of: impl Fn(&T) -> Cursor,
) -> Option<Cut> {
    debug_assert_eq!(rows.len(), json.len());
    let of = array_bytes(json);
    if of <= budget {
        return None;
    }
    let keep = rows_within(json, budget);
    if keep >= json.len() {
        return None;
    }
    rows.truncate(keep);
    json.truncate(keep);
    *next = rows.last().map(cursor_of);
    Some(Cut { bytes: array_bytes(json), of })
}

/// [`cut_page`] for a verb with no keyset cursor (`host.table.query`): drops
/// trailing `rows` until the array fits `budget`, at least one row staying.
/// The caller names the continuation, since only it knows what the next call
/// is.
pub fn cut_values(rows: &mut Vec<Value>, budget: u64) -> Option<Cut> {
    let of = array_bytes(rows);
    if of <= budget {
        return None;
    }
    let keep = rows_within(rows, budget);
    if keep >= rows.len() {
        return None;
    }
    rows.truncate(keep);
    Some(Cut { bytes: array_bytes(rows), of })
}

/// Leave the cut marker on `value` for `handler::call_tool`. `value` already
/// holds `next_cursor` (written by `paged::finish_page` from the cursor
/// [`cut_page`] set); `args` is the request's own arguments.
pub fn note_truncated(value: &mut Value, cut: Option<Cut>, verb: &str, args: &Value) {
    let Some(cut) = cut else { return };
    let Some(obj) = value.as_object() else { return };
    let mut next_args = match args {
        Value::Object(_) => args.clone(),
        _ => json!({}),
    };
    let continue_with = obj.get("next_cursor").cloned().map(|cursor| {
        if let Some(map) = next_args.as_object_mut() {
            map.insert("cursor".to_string(), cursor);
        }
        Call { verb: verb.to_string(), args: next_args }
    });
    note_marker(value, cut, continue_with);
}

/// Leave the marker for a cut whose continuation the caller built itself.
pub fn note_marker(value: &mut Value, cut: Cut, continue_with: Option<Call>) {
    let Some(obj) = value.as_object_mut() else { return };
    let marker = Truncated { bytes: cut.bytes, of: cut.of, continue_with };
    obj.insert(TRUNCATED_KEY.to_string(), marker.to_json());
}

/// Remove and return the marker [`note_truncated`] left.
pub fn take_truncated(value: &mut Value) -> Option<Value> {
    value.as_object_mut()?.remove(TRUNCATED_KEY)
}
