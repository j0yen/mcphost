//! PRD-mcphost-surface-fluidity AC5 — Given the served `llms.txt` and a
//! live `tools/list`, When compared by the test, Then the tool name sets
//! are equal.
//!
//! `www/llms.txt`'s `## Tools` section (see `mcphost::llms_txt`) is
//! generated from the same descriptors `handler.rs`'s `list_tools` serves,
//! rather than hand-maintained prose that drifts (the 2026-09-09 audit
//! found 14 documented vs 18 live tenant tools). "Live tools/list" here
//! means the union across both auth states -- `signup` is visible only
//! pre-auth, and `host.spec_test` only once authenticated -- exactly the
//! set `mcphost::llms_txt::tenant_tool_names` builds.

mod common;
use common::{TempDataDir, TestServer, all_kinds_registry, signup};
use std::collections::BTreeSet;

const LLMS_TXT: &str = include_str!("../www/llms.txt");

/// Parse the generated `## Tools` section's backtick-quoted bullet list
/// into a name set. Fails loudly (empty set) if the markers or section are
/// missing, rather than silently passing an equality check against nothing.
fn parse_tools_section(content: &str) -> BTreeSet<String> {
    let start = content
        .find(mcphost::llms_txt::TOOLS_SECTION_START)
        .expect("www/llms.txt must have a <!-- tools:start --> marker -- run `mcphost llms-txt`");
    let end = content
        .find(mcphost::llms_txt::TOOLS_SECTION_END)
        .expect("www/llms.txt must have a <!-- tools:end --> marker -- run `mcphost llms-txt`");
    content[start..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("- `")?;
            rest.strip_suffix('`').map(str::to_string)
        })
        .collect()
}

#[tokio::test]
async fn llms_txt_tool_section_names_the_same_set_as_a_live_tools_list() {
    let envs_dir = TempDataDir::new();
    let server = TestServer::start_with_kinds(all_kinds_registry(&envs_dir.0)).await;

    // Union across both auth states, same as `tenant_tool_names` builds
    // in-process -- `signup` is anonymous-only, `host.spec_test` is
    // authenticated-only.
    let anon_client = common::McpClient::new(&server.base_url);
    let anon = anon_client.tools_list().await.expect("anonymous tools/list");
    let (_ns, key) = signup(&server.base_url, "Surface AC5 Tenant").await;
    let auth_client = common::McpClient::with_bearer(&server.base_url, &key);
    let auth = auth_client.tools_list().await.expect("authenticated tools/list");

    let mut live: BTreeSet<String> = BTreeSet::new();
    for result in [&anon, &auth] {
        for tool in result["tools"].as_array().expect("tools array") {
            live.insert(
                tool["name"]
                    .as_str()
                    .expect("tool name is a string")
                    .to_string(),
            );
        }
    }
    // The newly-published tenant's own namespaced tool (there is none here
    // -- nothing was published) would also appear in `auth`; nothing to
    // filter since this tenant published nothing.

    let documented = parse_tools_section(LLMS_TXT);

    let missing_from_llms_txt: Vec<&String> = live.difference(&documented).collect();
    let extra_in_llms_txt: Vec<&String> = documented.difference(&live).collect();
    assert!(
        missing_from_llms_txt.is_empty() && extra_in_llms_txt.is_empty(),
        "www/llms.txt's tool section has drifted from a live tools/list -- run `mcphost llms-txt` \
         to regenerate. missing from llms.txt: {missing_from_llms_txt:?}; extra in llms.txt (no \
         longer live): {extra_in_llms_txt:?}"
    );

    // The PRD's own audit named these four as missing before this fix.
    for name in [
        "host.whoami",
        "host.bridge_test",
        "host.registry_publish",
        "host.tool_run",
    ] {
        assert!(
            documented.contains(name),
            "www/llms.txt must document {name}: {documented:?}"
        );
    }
}

/// The generator itself (`mcphost::llms_txt::tenant_tool_names`), run
/// against the same in-process descriptors, agrees with what's checked
/// into www/llms.txt -- proves the checked-in file isn't just accidentally
/// consistent with today's live server but actually IS what the generator
/// would produce right now.
#[test]
fn checked_in_llms_txt_matches_the_generator() {
    let kinds = mcphost::kinds::KindRegistry::with_builtin();
    let generated: BTreeSet<String> = mcphost::llms_txt::tenant_tool_names(&kinds)
        .into_iter()
        .collect();
    let documented = parse_tools_section(LLMS_TXT);
    assert_eq!(
        generated, documented,
        "www/llms.txt's tool section is stale -- run `mcphost llms-txt` to regenerate"
    );
}
