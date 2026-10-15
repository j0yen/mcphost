//! PRD-mcphost-help-search-tool-b
//! AC2 (P0) — Given the anonymous `tools/list` and a keyed `tools/list`, When
//! read, Then both contain `host.help.search`, `www/llms.txt`'s generated
//! tools section lists it, and the drift test over `docs/**/*.md` vs the
//! `build.rs` manifest passes.

use crate::common;
use common::{McpClient, TestServer};
use mcphost::kinds::KindRegistry;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn names(list: &serde_json::Value) -> Vec<String> {
    list["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("tool name").to_string())
        .collect()
}

#[tokio::test]
async fn hsrb_ac02_anonymous_and_keyed_tools_list_both_contain_help_search() {
    let server = TestServer::start().await;
    let anon = McpClient::new(&server.base_url);
    let anon_names = names(&anon.tools_list().await.expect("anonymous tools/list"));
    assert!(anon_names.iter().any(|n| n == "host.help.search"), "anonymous: {anon_names:?}");

    let (_tenant, key) = common::signup(&server.base_url, "hsrb-ac02").await;
    let keyed = McpClient::with_bearer(&server.base_url, &key);
    let keyed_names = names(&keyed.tools_list().await.expect("keyed tools/list"));
    assert!(keyed_names.iter().any(|n| n == "host.help.search"), "keyed: {keyed_names:?}");
}

#[test]
fn hsrb_ac02_llms_txt_generated_tools_section_lists_help_search() {
    let section = mcphost::llms_txt::render_tools_section(&KindRegistry::with_builtin());
    assert!(section.contains("`host.help.search`"), "generated section lacks host.help.search");
    let committed = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("www/llms.txt")).unwrap();
    assert!(committed.contains("- `host.help.search`"), "www/llms.txt lacks host.help.search");
}

fn md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read docs dir") {
        let path = entry.unwrap().path();
        if path.is_dir() {
            md_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

#[test]
fn hsrb_ac02_every_docs_md_on_disk_is_in_the_build_rs_manifest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut on_disk = Vec::new();
    md_files(&root.join("docs"), &mut on_disk);
    assert!(!on_disk.is_empty(), "docs/**/*.md found nothing");
    let manifest: BTreeSet<&str> = mcphost::help_search::HELP_DOCS.iter().map(|(p, _)| *p).collect();
    assert!(manifest.contains("README.md"), "README.md missing from the manifest");
    for path in on_disk {
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        assert!(manifest.contains(rel.as_str()), "{rel} is on disk but not in the build.rs manifest");
    }
}
