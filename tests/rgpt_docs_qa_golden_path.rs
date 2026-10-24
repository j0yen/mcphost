//! GENERATED from recipes/docs-qa.toml by `cargo xtask recipes` -- do not hand-edit.
//! PRD-mcphost-recipe-golden-path-tests R4: the `docs-qa` recipe's exact calls, in
//! order, against an in-process `TestServer` with the recipe's fixture routes
//! mounted; every `expect` must hold and the run must finish under 90 s.

use crate::rgpt;

const RECIPE: &str = r####"{
  "budget": {
    "calls": 6,
    "wall_s": 90
  },
  "fixture_routes": [],
  "https_public": false,
  "name": "docs-qa",
  "registry": "python",
  "steps": [
    {
      "args": {
        "content": "# VPN setup\n\nThe VPN client connects on port 1194 over UDP.\n",
        "name": "vpn-setup.md"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.docs.put"
    },
    {
      "args": {
        "content": "# Onboarding\n\nOn day one you receive a laptop and a badge.\n",
        "name": "onboarding.md"
      },
      "as": "owner",
      "expect": [],
      "retry_s": null,
      "signed_post": null,
      "tool": "host.docs.put"
    },
    {
      "args": {},
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.index.lag_seconds",
          "value": 0
        }
      ],
      "retry_s": 45,
      "signed_post": null,
      "tool": "host.docs.status"
    },
    {
      "args": {
        "kind": "python",
        "name": "ask_docs",
        "spec": {
          "source": "${file:examples/docs-qa/ask_docs.py}"
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
        "args": {
          "k": 5,
          "query": "What port does the VPN client connect on?"
        },
        "name": "ask_docs"
      },
      "as": "owner",
      "expect": [
        {
          "contains": null,
          "exists": null,
          "path": "$.passages[0].name",
          "value": "vpn-setup.md"
        }
      ],
      "retry_s": 45,
      "signed_post": null,
      "tool": "host.tool_call"
    }
  ],
  "tenants": [
    "owner"
  ]
}"####;

#[tokio::test]
async fn rgpt_docs_qa_golden_path() {
    if mcphost::sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", mcphost::sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let spec: serde_json::Value = serde_json::from_str(RECIPE).unwrap_or_else(|e| panic!("embedded recipe json: {e}"));
    rgpt::run(&spec).await;
}
