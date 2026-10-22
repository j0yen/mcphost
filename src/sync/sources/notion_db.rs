//! PRD-mcphost-sync-pull-with-change-detection R6: the `notion_db` source --
//! `POST /v1/databases/<id>/query` for `ref: "<database id>"`, paginated 100
//! per call (each page is one provider call) with the tenant's connected
//! token. A row is the page's properties flattened to text plus its `page_id`.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::pull::{Cursor, MAX_PAGES, Row, Snapshot, SourceCtx, SourceError, cell_text, sanitize_column, send_capped};

/// The provider's own API base.
pub const DEFAULT_BASE: &str = "https://api.notion.com";
/// The API version the property shapes below belong to.
pub const NOTION_VERSION: &str = "2022-06-28";

/// A Notion property value as table text: rich text joined, a select's name,
/// a number or checkbox as text; any other type as the JSON of its value.
pub fn property_text(prop: &Value) -> Value {
    let Some(ty) = prop.get("type").and_then(Value::as_str) else { return Value::Null };
    let value = &prop[ty];
    let joined = |items: &Value, key: &str| -> Value {
        let parts: Vec<&str> = items
            .as_array()
            .map(|a| a.iter().filter_map(|i| i.get(key).and_then(Value::as_str)).collect())
            .unwrap_or_default();
        Value::String(parts.join(if key == "name" { "," } else { "" }))
    };
    match ty {
        "title" | "rich_text" => joined(value, "plain_text"),
        "select" | "status" => value.get("name").map_or(Value::Null, cell_text),
        "multi_select" => joined(value, "name"),
        "people" => joined(value, "name"),
        "date" => value.get("start").map_or(Value::Null, cell_text),
        "checkbox" => Value::String(value.as_bool().unwrap_or(false).to_string()),
        _ => cell_text(value),
    }
}

/// Fetches every page of the database and returns its pages as rows.
pub async fn fetch(ctx: &SourceCtx<'_>, cursor: &Cursor) -> Result<Snapshot, SourceError> {
    let token = ctx.token.ok_or_else(|| SourceError::Failed("notion_db needs a connected notion account".to_string()))?;
    let database = ctx.reference.trim();
    if database.is_empty() {
        return Err(SourceError::Failed("notion_db ref must be a database id".to_string()));
    }
    let mut snap = Snapshot::default();
    let mut hasher = Sha256::new();
    let mut start_cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = reqwest::Url::parse(ctx.api_base.unwrap_or(DEFAULT_BASE))
            .map_err(|e| SourceError::Failed(format!("notion api base: {e}")))?;
        url.path_segments_mut()
            .map_err(|()| SourceError::Failed("notion api base cannot take a path".to_string()))?
            .pop_if_empty()
            .extend(["v1", "databases", database, "query"]);
        let mut body = json!({"page_size": 100});
        if let Some(c) = &start_cursor {
            body["start_cursor"] = json!(c);
        }
        let req = ctx.client.post(url).bearer_auth(token).header("Notion-Version", NOTION_VERSION).json(&body);
        let page = send_capped(req).await?;
        snap.provider_calls += 1;
        snap.bytes += page.body.len() as i64;
        hasher.update(&page.body);
        let doc: Value = serde_json::from_slice(&page.body)
            .map_err(|e| SourceError::Failed(format!("notion_db: the response is not JSON: {e}")))?;
        for result in doc.get("results").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            let mut row = Row::new();
            row.insert("page_id".to_string(), result.get("id").map_or(Value::Null, cell_text));
            if let Some(props) = result.get("properties").and_then(Value::as_object) {
                for (name, prop) in props {
                    row.insert(sanitize_column(name), property_text(prop));
                }
            }
            snap.rows.push(row);
        }
        let more = doc.get("has_more").and_then(Value::as_bool).unwrap_or(false);
        start_cursor = doc.get("next_cursor").and_then(Value::as_str).map(str::to_string);
        if !more || start_cursor.is_none() {
            break;
        }
    }
    let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    snap.unchanged = cursor.body_hash.as_deref() == Some(hash.as_str());
    snap.body_hash = Some(hash);
    Ok(snap)
}
