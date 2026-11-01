//! PRD-mcphost-table-import-from-url
//! AC6 (P0) — Given the error-codes registry, When the import codes are added,
//! Then the generated contract and both SDKs carry them and the registry drift
//! test passes with no hand edit to the contract.
//!
//! The five codes are `err_code!` declarations (src/table/import.rs); nothing
//! below lists them by hand except the expectation of which five they are. The
//! generated surfaces are compared with their generators' output, so a hand
//! edit or a stale file fails here.

use crate::common;

use common::sypul::Fixture;
use common::{McpClient, TestServer, signup};
use mcphost::error_registry::{ERROR_CODES, catalog_json, lookup};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

const CODES: &[&str] = &["import_fetch_failed", "import_parse_failed", "import_schema_mismatch", "import_too_large", "import_url_forbidden"];

fn read(rel: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

#[test]
fn tblurl_ac06_the_five_codes_are_registry_rows_with_their_data_keys() {
    let catalog = catalog_json("http://localhost");
    for code in CODES {
        let row = lookup(code).unwrap_or_else(|| panic!("{code} is a registry row"));
        assert!(!row.data_keys.is_empty(), "{code} declares the data keys it carries");
        assert!(row.remedy.len() <= 200 && !row.remedy.is_empty(), "{code}: remedy is one imperative sentence");
        let in_catalog = catalog["codes"].as_array().unwrap().iter().find(|r| r["code"] == *code).unwrap_or_else(|| panic!("{code} in /errors.json"));
        assert_eq!(in_catalog["data_keys"], json!(row.data_keys), "{code}");
    }
    assert!(ERROR_CODES.iter().filter(|e| CODES.contains(&e.code)).count() == CODES.len());
    // The retry policy is a registry property too: only a fetch failure is worth retrying.
    assert!(lookup("import_fetch_failed").unwrap().retryable);
    for code in ["import_parse_failed", "import_schema_mismatch", "import_too_large", "import_url_forbidden"] {
        assert!(!lookup(code).unwrap().retryable, "{code}");
    }
}

#[test]
fn tblurl_ac06_contract_docs_and_both_sdks_are_the_generators_output() {
    let errors_md = read("docs/errors.md");
    let py = read("sdk/python/mcphost_sdk/errors.py");
    let ts = read("sdk/ts/src/errors.ts");
    for code in CODES {
        let row = lookup(code).unwrap();
        for key in row.data_keys {
            assert!(errors_md.contains(&format!("`{key}`")), "docs/errors.md lacks {key} of {code}");
            assert!(py.contains(&format!("\"{key}\"")), "python SDK lacks {key} of {code}");
            assert!(ts.contains(&format!("\"{key}\"")), "ts SDK lacks {key} of {code}");
        }
        assert!(py.contains(&format!("\"{code}\"")), "python SDK lacks {code}");
        assert!(ts.contains(&format!("\"{code}\"")), "ts SDK lacks {code}");
    }
    assert_eq!(errors_md, mcphost::retry_policy::render_doc(), "docs/errors.md is stale");

    let contract = read("contracts/host-tools.v1.json");
    let built = mcphost::api_contract::dump_contract_bytes(&KindRegistry::with_builtin());
    assert_eq!(contract.as_bytes(), built.as_slice(), "contracts/host-tools.v1.json is not `mcphost contract dump`'s output");
    let py_files = mcphost::sdk_python::render(contract.as_bytes()).expect("render python sdk");
    assert_eq!(py_files["errors.py"], py, "sdk/python errors.py is stale");
    let ts_files = mcphost::r#gen::sdk_ts::render(contract.as_bytes()).expect("render ts sdk");
    assert_eq!(ts_files["errors.ts"], ts, "sdk/ts errors.ts is stale");
}

#[tokio::test]
async fn tblurl_ac06_every_raised_code_carries_the_keys_its_row_declares() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "TBLURL AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client.tools_call("host.table.create", json!({"name": "t", "columns": {"id": "integer"}})).await.expect("create");
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("lookup").expect("tenant");
    let mut strict = (*server.state).clone();
    strict.public_url = "https://mcphost.example".to_string();
    let missing = Fixture::start("id\n1\n", false).await;
    missing.set_status(404, None);

    let raised = |code: &str, data: &Value| {
        let row = lookup(code).unwrap();
        for key in row.data_keys {
            assert!(data.get(key).is_some(), "{code}: data.{key} is declared but was not raised: {data}");
        }
    };
    let forbidden = mcphost::tables::table_append(&strict, &tenant, &json!({"table": "t", "source_url": "https://10.0.0.1/x.csv"})).await.expect_err("forbidden");
    let fetch = client.tools_call("host.table.append", json!({"table": "t", "source_url": missing.csv_url()})).await.expect_err("fetch failed");
    let parse = client.tools_call("host.table.append", json!({"table": "t", "csv_text": "id\n\"open\n"})).await.expect_err("parse");
    let mismatch = client.tools_call("host.table.append", json!({"table": "t", "csv_text": "nope\n1\n"})).await.expect_err("mismatch");
    let big = format!("id\n{}", "1\n".repeat(600_000));
    let large = mcphost::tables::table_append(&strict, &tenant, &json!({"table": "t", "csv_text": big})).await.expect_err("too large");

    let data_of = |e: mcphost::errors::AppError| match e {
        mcphost::errors::AppError::Structured { code, data, .. } => (code, data),
        other => panic!("{other:?}"),
    };
    let (code, data) = data_of(forbidden);
    assert_eq!(code, "import_url_forbidden");
    raised(code, &data);
    assert_eq!(fetch.error_code.as_deref(), Some("import_fetch_failed"), "{fetch:?}");
    assert_eq!(fetch.data["status"], json!(404), "{:?}", fetch.data);
    for key in ["host", "status"] {
        assert!(fetch.data.get(key).is_some(), "import_fetch_failed: data.{key}");
    }
    assert_eq!(parse.error_code.as_deref(), Some("import_parse_failed"), "{parse:?}");
    assert!(parse.data.get("line").is_some(), "{:?}", parse.data);
    assert_eq!(mismatch.error_code.as_deref(), Some("import_schema_mismatch"), "{mismatch:?}");
    for key in ["expected", "unknown_columns", "rows_skipped", "skip_reasons"] {
        assert!(mismatch.data.get(key).is_some(), "import_schema_mismatch: data.{key}");
    }
    let (code, data) = data_of(large);
    assert_eq!(code, "import_too_large");
    assert!(data.get("bytes").is_some(), "{data}");
}
