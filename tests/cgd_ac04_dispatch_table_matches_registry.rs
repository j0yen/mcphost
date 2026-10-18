//! PRD-mcphost-capabilities-generated-from-dispatch
//! AC4 (P0) -- Given an exhaustive `match` over `rmcp::model::ClientRequest`,
//! When the test maps each variant to its method string, Then every method is
//! in `CORE_METHODS` or owned by exactly one row, and every row method outside
//! `server_to_client` is a `ClientRequest` method; Given a row method
//! `widgets/list` that rmcp cannot route, Then the test fails naming it.
//!
//! `ClientRequest` is `#[non_exhaustive]`, so the compiler cannot force a new
//! variant into [`variant_method`]; the guard for that is the deserialization
//! round trip in the second test (every row method must land in a typed
//! variant, never in the `CustomRequest` catch-all).

use mcphost::capabilities::{CAPABILITIES, CORE_METHODS};
use rmcp::model::{
    CallToolRequestMethod, CancelTaskMethod, ClientRequest, CompleteRequestMethod, ConstString, DiscoverRequestMethod,
    GetPromptRequestMethod, GetTaskMethod, InitializeResultMethod, ListPromptsRequestMethod,
    ListResourceTemplatesRequestMethod, ListResourcesRequestMethod, ListToolsRequestMethod, PingRequestMethod,
    ReadResourceRequestMethod, SetLevelRequestMethod, SubscribeRequestMethod, SubscriptionsListenRequestMethod,
    UnsubscribeRequestMethod, UpdateTaskMethod,
};
use serde_json::{Value, json};

/// The method string of each `ClientRequest` variant (`None` for the
/// `CustomRequest` catch-all). `_` is the `#[non_exhaustive]` arm.
fn variant_method(v: &ClientRequest) -> Option<&'static str> {
    Some(match v {
        ClientRequest::PingRequest(_) => PingRequestMethod::VALUE,
        ClientRequest::InitializeRequest(_) => InitializeResultMethod::VALUE,
        ClientRequest::DiscoverRequest(_) => DiscoverRequestMethod::VALUE,
        ClientRequest::CompleteRequest(_) => CompleteRequestMethod::VALUE,
        ClientRequest::SetLevelRequest(_) => SetLevelRequestMethod::VALUE,
        ClientRequest::GetPromptRequest(_) => GetPromptRequestMethod::VALUE,
        ClientRequest::ListPromptsRequest(_) => ListPromptsRequestMethod::VALUE,
        ClientRequest::ListResourcesRequest(_) => ListResourcesRequestMethod::VALUE,
        ClientRequest::ListResourceTemplatesRequest(_) => ListResourceTemplatesRequestMethod::VALUE,
        ClientRequest::ReadResourceRequest(_) => ReadResourceRequestMethod::VALUE,
        ClientRequest::SubscriptionsListenRequest(_) => SubscriptionsListenRequestMethod::VALUE,
        ClientRequest::SubscribeRequest(_) => SubscribeRequestMethod::VALUE,
        ClientRequest::UnsubscribeRequest(_) => UnsubscribeRequestMethod::VALUE,
        ClientRequest::CallToolRequest(_) => CallToolRequestMethod::VALUE,
        ClientRequest::ListToolsRequest(_) => ListToolsRequestMethod::VALUE,
        ClientRequest::GetTaskRequest(_) => GetTaskMethod::VALUE,
        ClientRequest::UpdateTaskRequest(_) => UpdateTaskMethod::VALUE,
        ClientRequest::CancelTaskRequest(_) => CancelTaskMethod::VALUE,
        ClientRequest::CustomRequest(_) => return None,
        _ => panic!("a new rmcp ClientRequest variant: add it to variant_method and to RMCP_METHODS"),
    })
}

/// One entry per typed `ClientRequest` variant, same order as the match.
const RMCP_METHODS: &[&str] = &[
    PingRequestMethod::VALUE,
    InitializeResultMethod::VALUE,
    DiscoverRequestMethod::VALUE,
    CompleteRequestMethod::VALUE,
    SetLevelRequestMethod::VALUE,
    GetPromptRequestMethod::VALUE,
    ListPromptsRequestMethod::VALUE,
    ListResourcesRequestMethod::VALUE,
    ListResourceTemplatesRequestMethod::VALUE,
    ReadResourceRequestMethod::VALUE,
    SubscriptionsListenRequestMethod::VALUE,
    SubscribeRequestMethod::VALUE,
    UnsubscribeRequestMethod::VALUE,
    CallToolRequestMethod::VALUE,
    ListToolsRequestMethod::VALUE,
    GetTaskMethod::VALUE,
    UpdateTaskMethod::VALUE,
    CancelTaskMethod::VALUE,
];

/// A registry row as the checker sees it: (name, methods, server_to_client).
type Row<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);

/// Every disagreement between rmcp's dispatch table and `rows`, each naming
/// the method.
fn dispatch_problems(rows: &[Row<'_>], rmcp_methods: &[&str], core: &[&str]) -> Vec<String> {
    let mut problems = Vec::new();
    for m in rmcp_methods {
        let owners: Vec<&str> = rows.iter().filter(|(_, ms, _)| ms.contains(m)).map(|(n, _, _)| *n).collect();
        if !core.contains(m) && owners.len() != 1 {
            problems.push(format!("rmcp routes `{m}` but it is neither in CORE_METHODS nor owned by exactly one row (owners: {owners:?})"));
        }
    }
    for (name, methods, s2c) in rows {
        for m in methods.iter().filter(|m| !s2c.contains(m)) {
            if !rmcp_methods.contains(m) {
                problems.push(format!("row `{name}` method `{m}` is not a ClientRequest method rmcp can route"));
            }
        }
    }
    problems
}

fn registry_rows() -> Vec<Row<'static>> {
    CAPABILITIES.iter().map(|c| (c.name, c.methods, c.server_to_client)).collect()
}

#[test]
fn cgd_ac04_every_rmcp_method_is_core_or_owned_and_every_row_method_is_routable() {
    let problems = dispatch_problems(&registry_rows(), RMCP_METHODS, CORE_METHODS);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn cgd_ac04_a_row_method_rmcp_cannot_route_fails_naming_it() {
    let mut rows = registry_rows();
    rows.push(("widgets", &["widgets/list"], &[]));
    let problems = dispatch_problems(&rows, RMCP_METHODS, CORE_METHODS);
    assert!(problems.iter().any(|p| p.contains("widgets/list")), "{problems:?}");
}

/// Params that deserialize for each row method (the typed variants are
/// strict about their params); everything else takes `{}`.
fn params_for(method: &str) -> Value {
    match method {
        "tools/call" => json!({"name": "x"}),
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => json!({"uri": "mcphost://x"}),
        "prompts/get" => json!({"name": "x"}),
        "completion/complete" => {
            let body: Value = serde_json::from_str(include_str!("fixtures/clive_complete_keyless.json")).expect("fixture is JSON");
            body["params"].clone()
        }
        "logging/setLevel" => json!({"level": "info"}),
        _ => json!({}),
    }
}

#[test]
fn cgd_ac04_every_client_to_server_row_method_deserializes_to_its_own_typed_variant() {
    let mut problems = Vec::new();
    for row in CAPABILITIES {
        for m in row.methods.iter().filter(|m| !row.server_to_client.contains(m)) {
            let v: Result<ClientRequest, _> = serde_json::from_value(json!({"method": m, "params": params_for(m)}));
            match v.as_ref().map(variant_method) {
                Ok(Some(got)) if got == *m => {}
                other => problems.push(format!("row `{}` method `{m}` does not route to a typed ClientRequest variant: {other:?}", row.name)),
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    let widgets: ClientRequest = serde_json::from_value(json!({"method": "widgets/list", "params": {}})).expect("custom");
    assert_eq!(variant_method(&widgets), None, "widgets/list must fall to CustomRequest");
}
