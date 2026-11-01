//! GENERATED from recipes/webhook-inbox.toml by `cargo xtask recipes` -- do not hand-edit.
//! PRD-mcphost-recipe-golden-path-tests R4: the `webhook-inbox` recipe's exact calls, in
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
  "name": "webhook-inbox",
  "registry": "python",
  "steps": [
    {
      "args": {
        "name": "inbox"
      },
      "as": "owner",
      "expect": [
        {
          "contains": "/hook/",
          "exists": null,
          "path": "$.url",
          "value": null
        },
        {
          "contains": null,
          "exists": true,
          "path": "$.secret",
          "value": null
        },
        {
          "contains": null,
          "exists": null,
          "path": "$.wait.tool",
          "value": "host.msg.wait"
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.webhook_inbox.create"
    },
    {
      "args": {},
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": {
        "body": "{\"id\":\"evt-fixture-1\"}",
        "secret": "${s1.secret}",
        "signed": true,
        "status": 200,
        "url": "${s1.url}"
      },
      "tool": null
    },
    {
      "args": {},
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": {
        "body": "{\"id\":\"evt-fixture-1\"}",
        "secret": "${s1.secret}",
        "signed": false,
        "status": 401,
        "url": "${s1.url}"
      },
      "tool": null
    },
    {
      "args": {
        "args": {},
        "name": "inbox"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.rows[0].id",
          "value": "evt-fixture-1"
        }
      ],
      "retry_s": 30,
      "signed_post": null,
      "tool": "host.tool_call"
    },
    {
      "args": "${s1.wait.args}",
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.messages[0].data.event_id",
          "value": "evt-fixture-1"
        }
      ],
      "retry_s": 30,
      "signed_post": null,
      "tool": "host.msg.wait"
    }
  ],
  "tenants": [
    "owner"
  ]
}"####;

#[tokio::test]
async fn rgpt_webhook_inbox_golden_path() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!("embedded recipe json: {e}"));
    rgpt::run(&spec).await;
}
