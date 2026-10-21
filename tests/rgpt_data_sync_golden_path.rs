//! GENERATED from recipes/data-sync.toml by `cargo xtask recipes` -- do not hand-edit.
//! PRD-mcphost-recipe-golden-path-tests R4: the `data-sync` recipe's exact calls, in
//! order, against an in-process `TestServer` with the recipe's fixture routes
//! mounted; every `expect` must hold and the run must finish under 60 s.

use crate::rgpt;

const RECIPE: &str = r####"{
  "budget": {
    "calls": 5,
    "wall_s": 60
  },
  "fixture_routes": [],
  "https_public": false,
  "name": "data-sync",
  "registry": "http",
  "steps": [
    {
      "args": {
        "mode": "pull",
        "source": {
          "kind": "csv_url",
          "ref": "https://raw.githubusercontent.com/j0yen/mcphost/main/examples/data-sync/orders.csv"
        },
        "table": "orders"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": true,
          "path": "$.sync_id",
          "value": null
        },
        {
          "contains": null,
          "exists": true,
          "path": "$.next_run_unix",
          "value": null
        },
        {
          "contains": null,
          "exists": true,
          "path": "$.status",
          "value": null
        },
        {
          "contains": null,
          "exists": null,
          "path": "$.table",
          "value": "orders"
        },
        {
          "contains": null,
          "exists": null,
          "path": "$.existing",
          "value": false
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.sync.create"
    },
    {
      "args": {
        "sync_id": "${s1.sync_id}"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.table",
          "value": "orders"
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.sync.get"
    },
    {
      "args": {},
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.syncs[0].table",
          "value": "orders"
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.sync.list"
    },
    {
      "args": {
        "sync_id": "${s1.sync_id}"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.deleted",
          "value": true
        },
        {
          "contains": null,
          "exists": null,
          "path": "$.table_kept",
          "value": true
        }
      ],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.sync.delete"
    }
  ],
  "tenants": [
    "owner"
  ]
}"####;

#[tokio::test]
async fn rgpt_data_sync_golden_path() {
    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!("embedded recipe json: {e}"));
    rgpt::run(&spec).await;
}
