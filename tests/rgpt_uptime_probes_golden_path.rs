//! GENERATED from recipes/uptime-probes.toml by `cargo xtask recipes` -- do not hand-edit.
//! PRD-mcphost-recipe-golden-path-tests R4: the `uptime-probes` recipe's exact calls, in
//! order, against an in-process `TestServer` with the recipe's fixture routes
//! mounted; every `expect` must hold and the run must finish under 60 s.

use crate::rgpt;

const RECIPE: &str = r####"{
  "budget": {
    "calls": 3,
    "wall_s": 60
  },
  "fixture_routes": [
    {
      "body": "ok",
      "method": "GET",
      "path": "/health",
      "status": 200
    }
  ],
  "https_public": false,
  "name": "uptime-probes",
  "registry": "http",
  "steps": [
    {
      "args": {
        "name": "status",
        "urls": [
          "${fixture_base}/health",
          "${fixture_base}/health?second=1"
        ]
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.status_tool",
          "value": "status"
        },
        {
          "contains": null,
          "exists": true,
          "path": "$.schedule_id",
          "value": null
        },
        {
          "contains": null,
          "exists": null,
          "path": "$.first_result.rows[0].ok",
          "value": true
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.uptime.create"
    },
    {
      "args": {
        "args": {},
        "name": "status"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.tool_call"
    }
  ],
  "tenants": [
    "owner"
  ]
}"####;

#[tokio::test]
async fn rgpt_uptime_probes_golden_path() {
    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!("embedded recipe json: {e}"));
    rgpt::run(&spec).await;
}
