//! GENERATED from recipes/share-a-tool.toml by `cargo xtask recipes` -- do not hand-edit.
//! PRD-mcphost-recipe-golden-path-tests R4: the `share-a-tool` recipe's exact calls, in
//! order, against an in-process `TestServer` with the recipe's fixture routes
//! mounted; every `expect` must hold and the run must finish under 60 s.

use crate::rgpt;

const RECIPE: &str = r####"{
  "budget": {
    "calls": 8,
    "wall_s": 60
  },
  "fixture_routes": [
    {
      "body": {
        "available": [
          {
            "amount": 4200,
            "currency": "usd"
          }
        ],
        "object": "balance"
      },
      "method": "GET",
      "path": "/v1/balance",
      "status": 200
    }
  ],
  "https_public": false,
  "name": "share-a-tool",
  "registry": "http",
  "steps": [
    {
      "args": {
        "name": "upstream_key",
        "value": "sk_live_fixture_not_a_real_key"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.secret_set"
    },
    {
      "args": {
        "kind": "http",
        "name": "paid_api",
        "spec": {
          "headers": {
            "Authorization": "Bearer {{ secret.upstream_key }}"
          },
          "method": "GET",
          "response": "json",
          "url": "${fixture_base}/v1/balance"
        }
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.tool_publish"
    },
    {
      "args": {
        "name": "finance"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.group.create"
    },
    {
      "args": {
        "group": "finance",
        "name": "paid_api",
        "visibility": "group"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.tool_share"
    },
    {
      "args": {
        "name": "finance",
        "namespace": "${teammate.ns}"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.group.add"
    },
    {
      "args": {
        "args": {},
        "name": "${owner.ns}.paid_api"
      },
      "as": "teammate",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.payload.object",
          "value": "balance"
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.tool_call"
    }
  ],
  "tenants": [
    "owner",
    "teammate"
  ]
}"####;

#[tokio::test]
async fn rgpt_share_a_tool_golden_path() {
    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!("embedded recipe json: {e}"));
    rgpt::run(&spec).await;
}
