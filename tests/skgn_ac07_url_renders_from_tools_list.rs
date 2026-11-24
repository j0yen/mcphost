//! PRD-mcphost-plugin-skill-generated-from-registry
//! AC7 (P0) -- Given `--url <the mcp url of a running host> --section verbs`
//! against a local test host, When rendered, Then the output is derived from
//! that host's `tools/list` and differs from the registry render only by
//! verbs not yet deployed.

use crate::common::{TestServer, signup};
use mcphost::kinds::KindRegistry;
use mcphost::skill_gen;
use std::collections::BTreeSet;

fn lines(text: &str) -> BTreeSet<String> {
    text.lines().map(str::to_string).collect()
}

#[tokio::test]
async fn skgn_ac07_url_render_is_the_hosts_tools_list() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Skgn AC7 Tenant").await;
    let url = format!("{}/mcp", server.base_url);
    let kinds = KindRegistry::with_builtin();
    let registry = skill_gen::render_section("verbs", &kinds, None).unwrap();

    // Derived from the live tools/list: with the host current, it equals the registry render.
    let live = skill_gen::render_verbs_from_url(&url, Some(&key)).await.expect("render from url");
    assert_eq!(skill_gen::first_difference(&live, &registry), None, "a host serving the registry's own verbs renders the same section");

    // The renderer reads the listing: a host that has not deployed one verb yet renders
    // every line the registry does except that verb's family line.
    let listed = skill_gen::fetch_tool_names(&url, Some(&key)).await.expect("tools/list");
    assert!(listed.iter().any(|n| n == "host.table.create"), "listing is the real one");
    let older: Vec<String> = listed.iter().filter(|n| n.as_str() != "host.table.create" && n.as_str() != "host_table_create").cloned().collect();
    let starters = skill_gen::fetch_tool_names(&url, None).await.unwrap();
    let older_render = skill_gen::render_verbs(&older, &starters);
    let only_in_registry: Vec<String> = lines(&registry).difference(&lines(&older_render)).cloned().collect();
    let only_in_older: Vec<String> = lines(&older_render).difference(&lines(&registry)).cloned().collect();
    assert_eq!(only_in_registry.len(), 1, "{only_in_registry:?}");
    assert_eq!(only_in_older.len(), 1, "{only_in_older:?}");
    assert!(only_in_registry[0].starts_with("- `host.table.*` ") && only_in_registry[0].contains("create"));
    assert!(!only_in_older[0].contains("create,") && !only_in_older[0].contains(" create"), "{}", only_in_older[0]);

    // The command line does the same.
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["skill-gen", "--section", "verbs", "--url", &url])
        .env("MCPHOST_TENANT_KEY", &key)
        .output()
        .await
        .expect("run skill-gen --url");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(skill_gen::first_difference(&String::from_utf8_lossy(&out.stdout), &format!("{live}\n")), None);

    // Without a key the anonymous listing is only the starter set: refused, not rendered short.
    let err = skill_gen::render_verbs_from_url(&url, None).await.expect_err("anonymous listing is refused");
    assert!(err.contains("--key"), "{err}");
}
