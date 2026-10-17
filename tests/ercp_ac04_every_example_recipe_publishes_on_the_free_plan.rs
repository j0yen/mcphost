//! PRD-mcphost-example-recipes-conform-to-policy
//! AC4 — Given every embedded example, When the conformance test publishes
//! each recipe spec against a TestServer on the free plan, Then none is
//! refused; and Given a fixture example with `network: "public"`, Then the
//! test fails naming it and `plan_required`.

use crate::common;
use common::{McpClient, TempDataDir, TestServer, python_kind_registry, signup};
use mcphost::examples::EXAMPLES;
use serde_json::{Value, json};

/// One recipe tool: the same fields `examples/index.json` carries.
struct Recipe {
    example: String,
    tool: String,
    kind: String,
    source: String,
    network: Option<String>,
}

fn embedded_recipes() -> Vec<Recipe> {
    EXAMPLES
        .iter()
        .flat_map(|e| {
            e.tools.iter().map(|t| Recipe {
                example: e.name.to_string(),
                tool: t.name.to_string(),
                kind: t.kind.to_string(),
                source: t.source.to_string(),
                network: t.network.map(str::to_string),
            })
        })
        .collect()
}

/// Publish every recipe into one fresh free tenant (the signup default;
/// signup is rate-limited per source, so one tenant, not one per example --
/// tool names are unique across the embedded examples). `Err` names the
/// example, the tool and the error code.
async fn conform(client: &McpClient, recipes: &[Recipe]) -> Result<usize, String> {
    let mut published = 0;
    for r in recipes {
        let mut spec = json!({"source": r.source});
        if let Some(net) = &r.network {
            spec["network"] = Value::String(net.clone());
        }
        client
            .tools_call("host.tool_publish", json!({"name": r.tool, "kind": r.kind, "spec": spec}))
            .await
            .map_err(|e| {
                format!(
                    "example `{}` tool `{}` refused: {} ({})",
                    r.example,
                    r.tool,
                    e.error_code.as_deref().unwrap_or("no error_code"),
                    e.message
                )
            })?;
        published += 1;
    }
    Ok(published)
}

#[tokio::test]
async fn ercp_ac04() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;

    let (_ns, key) = signup(&server.base_url, "ercp ac04 free tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let recipes = embedded_recipes();
    assert!(!recipes.is_empty(), "the embedded index must carry recipe tools");
    let n = conform(&client, &recipes).await.unwrap_or_else(|msg| panic!("{msg}"));
    assert_eq!(n, recipes.len());

    // Control: a fixture example asking for network public is refused on the
    // free plan, and the failure names the example and plan_required.
    let fixture = [Recipe {
        example: "fixture-public".into(),
        tool: "probe".into(),
        kind: "python".into(),
        source: "def main(args):\n    return {}\n".into(),
        network: Some("public".into()),
    }];
    let err = conform(&client, &fixture).await.expect_err("a public-network recipe must fail the conformance test");
    assert!(err.contains("fixture-public") && err.contains("plan_required"), "{err}");
}
