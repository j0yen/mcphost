//! GENERATED from recipes/first-run.toml by `cargo xtask recipes` -- do not hand-edit.
//! PRD-mcphost-recipe-golden-path-tests R4: the `first-run` recipe's exact calls, in
//! order, against an in-process `TestServer` with the recipe's fixture routes
//! mounted; every `expect` must hold and the run must finish under 90 s.

use crate::rgpt;

const RECIPE: &str = r####"{
  "budget": {
    "calls": 4,
    "wall_s": 90
  },
  "fixture_routes": [],
  "https_public": false,
  "name": "first-run",
  "registry": "python",
  "steps": [
    {
      "args": {
        "kind": "python",
        "name": "table_note",
        "spec": {
          "source": "${file:examples/first-run/tools/table_note.py}"
        }
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.kind",
          "value": "python"
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.tool_publish"
    },
    {
      "args": {
        "args": {
          "note": "hello"
        },
        "name": "table_note"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.result.appended",
          "value": 1
        },
        {
          "contains": null,
          "exists": null,
          "path": "$.result.table",
          "value": "quickstart_notes"
        }
      ],
      "retry_s": 45,
      "signed_post": null,
      "tool": "host.tool_test"
    },
    {
      "args": {
        "kind": "schedule",
        "schedule": "0 * * * *",
        "tool": "table_note"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.trigger.set"
    }
  ],
  "tenants": [
    "owner"
  ]
}"####;

#[tokio::test]
async fn rgpt_first_run_golden_path() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!("embedded recipe json: {e}"));
    rgpt::run(&spec).await;
}
