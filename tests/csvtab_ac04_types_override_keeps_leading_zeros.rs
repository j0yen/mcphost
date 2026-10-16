//! PRD-mcphost-csv-to-table-one-call
//! AC4 -- Given `types: {zip: "text"}` on a column that would infer
//! `integer`, When loaded, Then `columns[zip].type` is `text` and `00123` is
//! stored verbatim.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

fn csv() -> String {
    // 19 plain integers and one leading-zero value: the column infers
    // integer (the 95 % vote), and `00123` is the one cell it would refuse.
    let mut s = String::from("id,zip\n");
    for i in 1..=19 {
        s.push_str(&format!("{i},{}\n", 90000 + i));
    }
    s.push_str("20,00123\n");
    s
}

fn zip_type(report: &serde_json::Value) -> serde_json::Value {
    report["columns"].as_array().unwrap().iter().find(|c| c["name"] == "zip").unwrap()["type"].clone()
}

#[tokio::test]
async fn csvtab_ac04() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "CSVTAB AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.docs.put", json!({"name": "places.csv", "content": csv()}))
        .await
        .expect("put csv");

    let inferred = extract_structured(
        &client
            .tools_call("host.table.from_blob", json!({"doc": "places.csv", "name": "places_inferred"}))
            .await
            .expect("inferred load"),
    );
    assert_eq!(zip_type(&inferred), json!("integer"), "premise: zip infers integer: {inferred}");
    assert_eq!(inferred["rows_skipped"], json!(1), "the integer column refuses 00123: {inferred}");

    let forced = extract_structured(
        &client
            .tools_call(
                "host.table.from_blob",
                json!({"doc": "places.csv", "name": "places", "types": {"zip": "text"}}),
            )
            .await
            .expect("forced load"),
    );
    assert_eq!(zip_type(&forced), json!("text"), "{forced}");
    assert_eq!(forced["rows_loaded"], json!(20), "{forced}");
    assert_eq!(forced["rows_skipped"], json!(0), "{forced}");
    let id_type = forced["columns"].as_array().unwrap().iter().find(|c| c["name"] == "id").unwrap()["type"].clone();
    assert_eq!(id_type, json!("integer"), "only the named column is overridden: {forced}");

    let got = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT zip FROM places WHERE id = 20"}))
            .await
            .expect("query"),
    );
    assert_eq!(got["rows"][0]["zip"], json!("00123"), "{got}");
}
