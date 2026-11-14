//! contract: reads registry handler::host_tool_descriptors
//! PRD-mcphost-timeseries-buckets
//! AC3 -- Given `bucket="7s"` or `from="yesterday"`, When `host.ts.get` runs,
//! Then the refusal carries `allowed_buckets` and `time_forms` equal to the
//! consts the validator reads (one test parses both).

use crate::common::{McpClient, TestServer, signup};
use mcphost::errors::AppError;
use mcphost::handler::host_tool_descriptors;
use mcphost::kinds::KindRegistry;
use mcphost::timeseries::{ALLOWED_BUCKETS, TIME_FORMS};
use serde_json::{Value, json};

fn want_buckets() -> Vec<String> {
    ALLOWED_BUCKETS.iter().map(|(name, _)| (*name).to_string()).collect()
}

fn want_forms() -> Vec<String> {
    TIME_FORMS.iter().map(|f| (*f).to_string()).collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().unwrap_or_else(|| panic!("not an array: {v}")).iter().map(|s| s.as_str().expect("string").to_string()).collect()
}

/// The comma list after `marker` in a description, up to the next ` (`, `.` or ` (defaults`.
fn list_after(description: &str, marker: &str, stop: &str) -> Vec<String> {
    let tail = description.split(marker).nth(1).unwrap_or_else(|| panic!("no {marker:?} in {description}"));
    let list = tail.split(stop).next().expect("stop");
    list.split(',').map(|s| s.trim().to_string()).collect()
}

#[tokio::test]
async fn tsb_ac03_description_schema_and_refusals_all_carry_the_validator_consts() {
    // The descriptor: the tool description and the schema enum render the consts.
    let tools = host_tool_descriptors(&KindRegistry::with_builtin());
    let get = tools.iter().find(|t| t.name.as_ref() == "host.ts.get").expect("host.ts.get is registered");
    let description = get.description.as_deref().expect("description");
    assert_eq!(list_after(description, "bucket is one of ", ". from"), want_buckets(), "description buckets");
    assert_eq!(list_after(description, "from and to take ", " (defaults"), want_forms(), "description time forms");
    let schema = Value::Object((*get.input_schema).clone());
    assert_eq!(strings(&schema["properties"]["bucket"]["enum"]), want_buckets(), "schema enum");
    assert_eq!(strings(&schema["properties"]["bucket"]["x-mcphost-refusal-data"]["allowed_buckets"]), want_buckets());
    assert_eq!(strings(&schema["properties"]["from"]["x-mcphost-refusal-data"]["time_forms"]), want_forms());

    // The wire: the validator's refusal for a bad bucket and a bad time form.
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "tsb ac3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for args in [
        json!({"series": "latency_ms", "bucket": "7s"}),
        json!({"series": "latency_ms", "bucket": "1h", "from": "yesterday"}),
        json!({"series": "latency_ms", "bucket": "1h", "to": "tomorrow"}),
    ] {
        let err = client.tools_call("host.ts.get", args.clone()).await.expect_err("refused");
        assert_eq!(err.error_code.as_deref(), Some("args_invalid"), "{args}: {}", err.message);
        assert_eq!(strings(&err.data["allowed_buckets"]), want_buckets(), "{args}: {}", err.data);
        assert_eq!(strings(&err.data["time_forms"]), want_forms(), "{args}: {}", err.data);
    }

    // The handler (a call that reaches it without the validator, as the sandbox bridge does).
    let (ns, _) = signup(&server.base_url, "tsb ac3 handler").await;
    let tenant = server.state.db.find_tenant_by_namespace(ns).await.expect("db").expect("tenant");
    for args in [
        json!({"series": "latency_ms", "bucket": "7s"}),
        json!({"series": "latency_ms", "bucket": "1h", "from": "yesterday"}),
    ] {
        match mcphost::timeseries::get(&server.state, &tenant, &args).await {
            Err(AppError::Structured { code, data, .. }) => {
                assert_eq!(code, "args_invalid");
                assert_eq!(strings(&data["allowed_buckets"]), want_buckets(), "{args}");
                assert_eq!(strings(&data["time_forms"]), want_forms(), "{args}");
            }
            other => panic!("{args}: expected a structured refusal, got {other:?}"),
        }
    }
}
