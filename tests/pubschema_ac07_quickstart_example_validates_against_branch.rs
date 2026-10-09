//! PRD-mcphost-publish-schema-from-registry
//! AC7 (P2) -- Given `host.quickstart {kind: <k>}` for each registered
//! kind, When its example `spec` is validated against the generated
//! branch for `<k>`, Then it validates.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, extract_structured, five_kinds_registry, signup};
use serde_json::json;

#[tokio::test]
async fn every_kinds_quickstart_example_validates_against_its_published_branch() {
    let envs_dir = TempDataDir::new();
    let registry = five_kinds_registry(&envs_dir.0);
    let names: Vec<&'static str> = registry.names();
    let server = TestServer::start_with_kinds(registry).await;
    let (_ns, key) = signup(&server.base_url, "Pubschema AC7 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let listed = client.tools_list().await.expect("tools/list");
    let publish = listed["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == "host.tool_publish")
        .expect("host.tool_publish is listed");
    let validator = jsonschema::validator_for(&publish["inputSchema"]).expect("compile published schema");

    assert_eq!(names.len(), 5);
    for name in names {
        let quickstart = extract_structured(
            &client
                .tools_call("host.quickstart", json!({"kind": name}))
                .await
                .unwrap_or_else(|e| panic!("quickstart {name}: {e:?}")),
        );
        let call = &quickstart["steps"][0]["arguments"];
        assert_eq!(call["kind"], json!(name), "{quickstart}");
        let spec = &call["spec"];
        assert!(spec.is_object(), "{name} example spec: {spec}");

        // The whole publish call (name + kind + spec) against the whole
        // schema, so the `if kind == name` branch is the one that applies.
        let instance = json!({"name": "my_tool", "kind": name, "spec": spec});
        let errors: Vec<String> = validator.iter_errors(&instance).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{name} example does not validate: {errors:?}\nspec: {spec}");

        // And the branch is genuinely selective: an unknown field fails.
        let mut bad = instance.clone();
        bad["spec"]["zzz_not_a_field"] = json!(1);
        assert!(!validator.is_valid(&bad), "{name} branch must reject an unknown spec field");
    }
}
