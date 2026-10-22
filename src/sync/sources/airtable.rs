//! PRD-mcphost-sync-pull-with-change-detection R6: the `airtable` source --
//! `GET /v0/<base>/<table>` for `ref: "<base>/<table>"`, paginated 100 per
//! call (each page is one provider call) with the tenant's connected token.
//! A row is the record's `fields` plus its `record_id`.

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::super::pull::{
    Cursor, MAX_PAGES, Row, Snapshot, SourceCtx, SourceError, cell_text, sanitize_column, send_capped,
};

/// The provider's own API base.
pub const DEFAULT_BASE: &str = "https://api.airtable.com";

/// Fetches every page and returns the records as rows.
pub async fn fetch(ctx: &SourceCtx<'_>, cursor: &Cursor) -> Result<Snapshot, SourceError> {
    let token = ctx.token.ok_or_else(|| SourceError::Failed("airtable needs a connected airtable account".to_string()))?;
    let (base_id, table_id) = ctx
        .reference
        .trim()
        .split_once('/')
        .filter(|(b, t)| !b.is_empty() && !t.is_empty())
        .ok_or_else(|| SourceError::Failed("airtable ref must be <base id>/<table id>".to_string()))?;
    let mut snap = Snapshot::default();
    let mut hasher = Sha256::new();
    let mut offset: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = reqwest::Url::parse(ctx.api_base.unwrap_or(DEFAULT_BASE))
            .map_err(|e| SourceError::Failed(format!("airtable api base: {e}")))?;
        url.path_segments_mut()
            .map_err(|()| SourceError::Failed("airtable api base cannot take a path".to_string()))?
            .pop_if_empty()
            .extend(["v0", base_id, table_id]);
        url.query_pairs_mut().append_pair("pageSize", "100");
        if let Some(o) = &offset {
            url.query_pairs_mut().append_pair("offset", o);
        }
        let page = send_capped(ctx.client.get(url).bearer_auth(token)).await?;
        snap.provider_calls += 1;
        snap.bytes += page.body.len() as i64;
        hasher.update(&page.body);
        let doc: Value = serde_json::from_slice(&page.body)
            .map_err(|e| SourceError::Failed(format!("airtable: the response is not JSON: {e}")))?;
        for record in doc.get("records").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            let mut row = Row::new();
            row.insert("record_id".to_string(), record.get("id").map_or(Value::Null, cell_text));
            if let Some(fields) = record.get("fields").and_then(Value::as_object) {
                for (k, v) in fields {
                    row.insert(sanitize_column(k), cell_text(v));
                }
            }
            snap.rows.push(row);
        }
        offset = doc.get("offset").and_then(Value::as_str).map(str::to_string);
        if offset.is_none() {
            break;
        }
    }
    let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    snap.unchanged = cursor.body_hash.as_deref() == Some(hash.as_str());
    snap.body_hash = Some(hash);
    Ok(snap)
}
