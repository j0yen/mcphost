//! PRD-mcphost-audit-export-tamper-evident
//! AC7 (P1) — Given `format: "csv"`, When exported, Then the row count equals
//! the JSONL and the last row carries `root_hash`.

use crate::audx_ac02_export_nine_rows_three_tables::{parse_lines, seed_nine};
use crate::common::{TestServer, extract_structured, signup};
use serde_json::json;

/// A small RFC 4180 reader: quoted fields may hold commas, quotes and newlines.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let (mut records, mut record, mut field) = (Vec::new(), Vec::new(), String::new());
    let (mut quoted, mut chars) = (false, text.chars().peekable());
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, c) => field.push(c),
            (false, '"') => quoted = true,
            (false, ',') => record.push(std::mem::take(&mut field)),
            (false, '\n') => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            (false, c) => field.push(c),
        }
    }
    records
}

#[tokio::test]
async fn audx_ac07_csv_has_the_jsonl_row_count_and_the_last_row_carries_the_root() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Audx AC7").await;
    let (client, _) = seed_nine(&server, ns, &key).await;

    let jsonl = extract_structured(&client.tools_call("host.audit.export", json!({})).await.expect("jsonl export"));
    let csv = extract_structured(&client.tools_call("host.audit.export", json!({"format": "csv"})).await.expect("csv export"));
    assert!(csv.get("jsonl").is_none(), "{csv}");
    let records = parse_csv(csv["csv"].as_str().expect("csv"));
    let columns = &records[0];
    assert_eq!(columns[0], "ts");
    let col = |name: &str| columns.iter().position(|c| c == name).unwrap_or_else(|| panic!("column {name}: {columns:?}"));

    // Header row, one row per audit line, trailer row.
    let (data, trailer) = (&records[1..records.len() - 1], records.last().expect("trailer"));
    let json_lines = parse_lines(jsonl["jsonl"].as_str().expect("jsonl"));
    let json_rows = &json_lines[1..json_lines.len() - 1];
    assert_eq!(data.len(), json_rows.len(), "row count equals the JSONL");
    assert_eq!(data.len(), 9);
    for (c, j) in data.iter().zip(json_rows) {
        assert_eq!(c[col("hash")], j["hash"].as_str().unwrap());
        assert_eq!(c[col("table")], j["table"].as_str().unwrap());
        // The quoted detail (a comma-free string with quotes, or a JSON object) survives the CSV quoting.
        assert_eq!(serde_json::from_str::<serde_json::Value>(&c[col("detail")]).expect("detail json"), j["detail"]);
    }

    // The last row carries root_hash (and the count and signature), and it is the JSONL's root.
    assert_eq!(trailer[col("root_hash")], json_lines.last().unwrap()["root_hash"].as_str().unwrap());
    assert_eq!(trailer[col("id")], "9");
    assert_eq!(trailer[col("root_hash")], data.last().unwrap()[col("hash")]);
    assert_eq!(csv["root_hash"], jsonl["root_hash"]);
    assert!(!trailer[col("signature")].is_empty());
}
