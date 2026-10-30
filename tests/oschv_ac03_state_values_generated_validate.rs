//! PRD-mcphost-output-schema-tenant-values-opaque
//! AC3 (P0) -- Given 200 seeded random JSON values including arrays,
//! booleans, floats and nested objects, When each is stored with
//! `host.state.set` and read with `host.state.get`, `host.state.list` and
//! `host.state.query`, Then every `structuredContent` validates against its
//! tool's `outputSchema`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{Map, Value, json};

const SEED: u64 = 0x05C4_0A17;
const VALUES: usize = 200;

fn gen_value(rng: &mut StdRng, depth: u32) -> Value {
    let top = if depth >= 3 { 5 } else { 7 };
    match rng.gen_range(0..top) {
        0 => Value::Null,
        1 => Value::Bool(rng.gen_bool(0.5)),
        2 => json!(rng.gen_range(-1_000_000i64..1_000_000)),
        3 => json!(rng.gen_range(-1.0e6f64..1.0e6) + 0.5),
        4 => Value::String((0..rng.gen_range(0..12)).map(|_| rng.gen_range('a'..='z')).collect()),
        5 => Value::Array((0..rng.gen_range(0..4)).map(|_| gen_value(rng, depth + 1)).collect()),
        _ => {
            let mut m = Map::new();
            for i in 0..rng.gen_range(0..4) {
                m.insert(format!("k{i}"), gen_value(rng, depth + 1));
            }
            Value::Object(m)
        }
    }
}

async fn schema_of(client: &McpClient, tool: &str) -> Value {
    let listing = client.tools_list().await.expect("tools/list");
    listing["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == tool)
        .unwrap_or_else(|| panic!("{tool} not listed"))["outputSchema"]
        .clone()
}

#[allow(clippy::too_many_arguments)] // six distinct per-seed facts; a struct would only rename them
fn check(problems: &mut Vec<String>, schema: &Value, tool: &str, seed_index: usize, value: &Value, got: &Value) {
    let validator = jsonschema::validator_for(schema).expect("outputSchema compiles");
    if let Some(e) = validator.iter_errors(got).next() {
        problems.push(format!("#{seed_index} {tool} value={value}: {}: {e}", e.instance_path));
    }
}

#[tokio::test]
async fn two_hundred_generated_values_validate_through_set_get_list_and_query() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Oschv AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let get_schema = schema_of(&client, "host.state.get").await;
    let list_schema = schema_of(&client, "host.state.list").await;
    let query_schema = schema_of(&client, "host.state.query").await;

    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "vals", "schema": {"id": "integer", "value": "json"}, "primary_key": "id"}),
        )
        .await
        .expect("table_create");

    let mut rng = StdRng::seed_from_u64(SEED);
    let mut problems = Vec::new();
    for i in 0..VALUES {
        let value = gen_value(&mut rng, 0);
        let key_name = format!("k{i}");
        client
            .tools_call("host.state.set", json!({"key": key_name, "value": value}))
            .await
            .unwrap_or_else(|e| panic!("#{i} set {value}: {e:?}"));
        client
            .tools_call("host.state.insert", json!({"table": "vals", "rows": {"id": i, "value": value}}))
            .await
            .unwrap_or_else(|e| panic!("#{i} insert {value}: {e:?}"));

        let got = extract_structured(
            &client.tools_call("host.state.get", json!({"key": key_name})).await.expect("get"),
        );
        check(&mut problems, &get_schema, "host.state.get", i, &value, &got);

        let listed = extract_structured(
            &client.tools_call("host.state.list", json!({"prefix": key_name})).await.expect("list"),
        );
        check(&mut problems, &list_schema, "host.state.list", i, &value, &listed);

        let queried = extract_structured(
            &client
                .tools_call("host.state.query", json!({"table": "vals", "where": format!("id = {i}")}))
                .await
                .expect("query"),
        );
        check(&mut problems, &query_schema, "host.state.query", i, &value, &queried);
    }
    assert!(problems.is_empty(), "{} failures (seed {SEED:#x}):\n{}", problems.len(), problems.join("\n"));
}
