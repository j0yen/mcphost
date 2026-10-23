//! GENERATED from recipes/team-memory.toml by `cargo xtask recipes` -- do not hand-edit.
//! PRD-mcphost-recipe-golden-path-tests R4: the `team-memory` recipe's exact calls, in
//! order, against an in-process `TestServer` with the recipe's fixture routes
//! mounted; every `expect` must hold and the run must finish under 90 s.

use crate::rgpt;

const RECIPE: &str = r####"{
  "budget": {
    "calls": 11,
    "wall_s": 90
  },
  "fixture_routes": [],
  "https_public": false,
  "name": "team-memory",
  "registry": "python",
  "steps": [
    {
      "args": {
        "columns": {
          "at": "real",
          "key": "text",
          "tags": "json",
          "text": "text",
          "writer": "text"
        },
        "name": "memory"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.table.create"
    },
    {
      "args": {
        "kind": "python",
        "name": "remember",
        "spec": {
          "source": "${file:examples/team-memory/tools/remember.py}"
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
        "kind": "python",
        "name": "recall",
        "spec": {
          "source": "${file:examples/team-memory/tools/recall.py}"
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
        "name": "team"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.group.create"
    },
    {
      "args": {
        "group": "team",
        "name": "remember",
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
        "group": "team",
        "name": "recall",
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
        "name": "team",
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
        "args": {
          "key": "deploy",
          "text": "deploy window is Tuesday 9pm",
          "who": "${teammate.ns}"
        },
        "name": "${owner.ns}.remember"
      },
      "as": "teammate",
      "expect": [],
      "retry_s": 45,
      "signed_post": null,
      "tool": "host.tool_call"
    },
    {
      "args": {
        "args": {
          "query": "deploy"
        },
        "name": "${owner.ns}.recall"
      },
      "as": "teammate",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.rows[0].key",
          "value": "deploy"
        }
      ],
      "retry_s": 45,
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
async fn rgpt_team_memory_golden_path() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!("embedded recipe json: {e}"));
    rgpt::run(&spec).await;
}
