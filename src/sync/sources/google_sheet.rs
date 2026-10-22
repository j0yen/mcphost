//! PRD-mcphost-sync-pull-with-change-detection R6: the `google_sheet` source
//! -- Sheets `values.get` for `ref: "<sheet id>/<tab>"` (the sheet's own URL
//! also works, and a bare id reads `Sheet1`) with the tenant's connected
//! Google token. The first row of `values` names the columns.

use serde_json::Value;

use super::super::pull::{Cursor, Row, Snapshot, SourceCtx, SourceError, cell_text, conditional, send_capped, snapshot_of, unique_columns};

/// The provider's own API base.
pub const DEFAULT_BASE: &str = "https://sheets.googleapis.com";
/// The tab a ref without one reads.
pub const DEFAULT_TAB: &str = "Sheet1";

/// `(spreadsheet id, tab)` from a `ref`: `<id>`, `<id>/<tab>`, or a
/// `https://docs.google.com/spreadsheets/d/<id>/...` url.
pub fn parse_ref(reference: &str) -> (String, String) {
    let reference = reference.trim();
    if reference.starts_with("http")
        && let Some((_, rest)) = reference.split_once("/d/")
    {
        let id = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        return (id.to_string(), DEFAULT_TAB.to_string());
    }
    match reference.split_once('/') {
        Some((id, tab)) if !tab.is_empty() => (id.to_string(), tab.to_string()),
        Some((id, _)) => (id.to_string(), DEFAULT_TAB.to_string()),
        None => (reference.to_string(), DEFAULT_TAB.to_string()),
    }
}

/// Fetches the tab and turns `values` into rows.
pub async fn fetch(ctx: &SourceCtx<'_>, cursor: &Cursor) -> Result<Snapshot, SourceError> {
    let token = ctx.token.ok_or_else(|| SourceError::Failed("google_sheet needs a connected google account".to_string()))?;
    let (id, tab) = parse_ref(ctx.reference);
    let mut url = reqwest::Url::parse(ctx.api_base.unwrap_or(DEFAULT_BASE))
        .map_err(|e| SourceError::Failed(format!("sheets api base: {e}")))?;
    url.path_segments_mut()
        .map_err(|()| SourceError::Failed("sheets api base cannot take a path".to_string()))?
        .pop_if_empty()
        .extend(["v4", "spreadsheets", id.as_str(), "values", tab.as_str()]);
    url.query_pairs_mut().append_pair("majorDimension", "ROWS");
    let fetched = send_capped(conditional(ctx.client.get(url).bearer_auth(token), cursor)).await?;
    let mut snap = snapshot_of(&fetched, cursor);
    if snap.unchanged {
        return Ok(snap);
    }
    let doc: Value = serde_json::from_slice(&fetched.body)
        .map_err(|e| SourceError::Failed(format!("google_sheet: the response is not JSON: {e}")))?;
    let values = doc.get("values").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut lines = values.iter().filter_map(Value::as_array);
    let Some(header) = lines.next() else { return Ok(snap) };
    let header: Vec<String> = header.iter().map(|h| h.as_str().map(str::to_string).unwrap_or_else(|| h.to_string())).collect();
    let columns = unique_columns(&header);
    snap.rows = lines
        .map(|line| {
            let mut row = Row::new();
            for (i, col) in columns.iter().enumerate() {
                row.insert(col.clone(), line.get(i).map_or(Value::Null, cell_text));
            }
            row
        })
        .collect();
    Ok(snap)
}
