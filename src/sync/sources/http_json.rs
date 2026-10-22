//! PRD-mcphost-sync-pull-with-change-detection R6: the `http_json` source --
//! a conditional GET of a public url answering a JSON array of objects. A
//! row's columns are its keys; a non-string value is stored as its JSON text.

use serde_json::Value;

use super::super::pull::{Cursor, Row, Snapshot, SourceCtx, SourceError, cell_text, conditional, sanitize_column, send_capped, snapshot_of};

/// Fetches `ctx.reference` and parses the array into a [`Snapshot`].
pub async fn fetch(ctx: &SourceCtx<'_>, cursor: &Cursor) -> Result<Snapshot, SourceError> {
    let fetched = send_capped(conditional(ctx.client.get(ctx.reference), cursor)).await?;
    let mut snap = snapshot_of(&fetched, cursor);
    if snap.unchanged {
        return Ok(snap);
    }
    let doc: Value = serde_json::from_slice(&fetched.body)
        .map_err(|e| SourceError::Failed(format!("http_json: the response is not JSON: {e}")))?;
    let items = doc
        .as_array()
        .ok_or_else(|| SourceError::Failed("http_json: expected a JSON array of objects".to_string()))?;
    for (i, item) in items.iter().enumerate() {
        let obj = item
            .as_object()
            .ok_or_else(|| SourceError::Failed(format!("http_json: element {i} is not an object")))?;
        let mut row = Row::new();
        for (k, v) in obj {
            row.insert(sanitize_column(k), cell_text(v));
        }
        snap.rows.push(row);
    }
    Ok(snap)
}
