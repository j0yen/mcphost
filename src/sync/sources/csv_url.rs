//! PRD-mcphost-sync-pull-with-change-detection R6: the `csv_url` source -- a
//! conditional GET of a public CSV (`If-None-Match`/`If-Modified-Since` from
//! the cursor), parsed with RFC 4180 quoting; the header row names the columns.

use super::super::pull::{Cursor, Row, Snapshot, SourceCtx, SourceError, conditional, send_capped, snapshot_of, unique_columns};
use serde_json::Value;

/// Fetches `ctx.reference` and parses it into a [`Snapshot`].
pub async fn fetch(ctx: &SourceCtx<'_>, cursor: &Cursor) -> Result<Snapshot, SourceError> {
    let fetched = send_capped(conditional(ctx.client.get(ctx.reference), cursor)).await?;
    let mut snap = snapshot_of(&fetched, cursor);
    if snap.unchanged {
        return Ok(snap);
    }
    let text = String::from_utf8_lossy(&fetched.body);
    snap.rows = parse_csv(&text);
    Ok(snap)
}

/// RFC 4180: quoted fields, `""` escapes, CRLF or LF, a leading BOM ignored.
/// The first record is the header; blank lines are skipped and short records
/// pad with null.
pub fn parse_csv(text: &str) -> Vec<Row> {
    let text = text.trim_start_matches('\u{feff}');
    let mut records: Vec<Vec<String>> = Vec::new();
    let (mut field, mut record) = (String::new(), Vec::<String>::new());
    let (mut quoted, mut any) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => quoted = false,
                other => field.push(other),
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                any = true;
            }
            ',' => {
                record.push(std::mem::take(&mut field));
                any = true;
            }
            '\r' => {}
            '\n' => {
                if any || !field.is_empty() {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                }
                any = false;
            }
            other => {
                field.push(other);
                any = true;
            }
        }
    }
    if any || !field.is_empty() {
        record.push(field);
        records.push(record);
    }
    let mut iter = records.into_iter();
    let Some(header) = iter.next() else { return Vec::new() };
    let columns = unique_columns(&header);
    iter.map(|rec| {
        let mut row = Row::new();
        for (i, col) in columns.iter().enumerate() {
            row.insert(col.clone(), rec.get(i).map_or(Value::Null, |v| Value::String(v.clone())));
        }
        row
    })
    .collect()
}
