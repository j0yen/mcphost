//! PRD-mcphost-sandbox-return-shape-contract
//! AC4 (P0) -- Given an envelope result, When a tool uses `result["rows"]`,
//! `result.get("rows")`, `"rows" in result`, `len(result)`, `result.items()`,
//! `result == {...}`, `json.dumps(result)` and `copy.deepcopy(result)`, Then
//! each behaves as a plain dict (one assertion each).

use crate::common;
use common::{TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;
use std::time::Duration;

const SOURCE: &str = r#"import copy
import json
import mcphost

def main(args):
    mcphost.table.create('t', {'n': 'integer'})
    mcphost.table.append('t', [{'n': 1}, {'n': 2}])
    result = mcphost.table.query('SELECT n FROM t ORDER BY n')
    rows = [{'n': 1}, {'n': 2}]
    return {
        'getitem': result['rows'],
        'get': result.get('rows'),
        'get_default': result.get('missing', 'dflt'),
        'contains': 'rows' in result,
        'len': len(result),
        'items': [[k, v] for k, v in result.items()],
        'keys': list(result.keys()),
        'eq_plain': result == {'rows': rows},
        'eq_reflected': {'rows': rows} == result,
        'json_dumps': json.loads(json.dumps(result)),
        'deepcopy': copy.deepcopy(result) == {'rows': rows},
        'deepcopy_independent': copy.deepcopy(result)['rows'] is not result['rows'],
        'dict_copy': dict(result),
        'unpack': {**result},
        'is_dict': isinstance(result, dict),
    }
"#;

#[tokio::test]
async fn an_envelope_result_behaves_as_a_plain_dict() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Retshape AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "plain_dict", "kind": "python", "spec": {"source": SOURCE}}),
        )
        .await
        .expect("publish ok");
    let r = extract_structured(
        &poll_until_ready(&client, &format!("{ns}.plain_dict"), json!({}), Duration::from_secs(20))
            .await
            .unwrap_or_else(|e| panic!("tool must run: {} {}", e.code, e.message)),
    );

    let rows = json!([{"n": 1}, {"n": 2}]);
    assert_eq!(r["getitem"], rows, "result[\"rows\"]: {r}");
    assert_eq!(r["get"], rows, "result.get(\"rows\"): {r}");
    assert_eq!(r["get_default"], json!("dflt"), "result.get(missing, default): {r}");
    assert_eq!(r["contains"], json!(true), "\"rows\" in result: {r}");
    assert_eq!(r["len"], json!(1), "len(result): {r}");
    assert_eq!(r["items"], json!([["rows", rows]]), "result.items(): {r}");
    assert_eq!(r["keys"], json!(["rows"]), "result.keys(): {r}");
    assert_eq!(r["eq_plain"], json!(true), "result == {{...}}: {r}");
    assert_eq!(r["eq_reflected"], json!(true), "{{...}} == result: {r}");
    assert_eq!(r["json_dumps"], json!({"rows": rows}), "json.dumps(result): {r}");
    assert_eq!(r["deepcopy"], json!(true), "copy.deepcopy(result): {r}");
    assert_eq!(r["deepcopy_independent"], json!(true), "deepcopy must copy rows: {r}");
    assert_eq!(r["dict_copy"], json!({"rows": rows}), "dict(result): {r}");
    assert_eq!(r["unpack"], json!({"rows": rows}), "{{**result}}: {r}");
    assert_eq!(r["is_dict"], json!(true), "isinstance(result, dict): {r}");
}
