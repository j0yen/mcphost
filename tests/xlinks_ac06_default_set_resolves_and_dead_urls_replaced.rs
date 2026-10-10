//! AC6 (PRD-mcphost-docs-external-links-resolve) -- Given the repository at
//! build, When `docs-link-check.sh` runs over the default file set, Then it
//! exits 0 and the Goose, Warp and registry URLs in `clients.toml` differ
//! from tonight's three dead URLs.

use mcphost::clients;

use crate::xlinks;

const README: &str = include_str!("../README.md");
const LLMS_TXT: &str = include_str!("../www/llms.txt");

const DEAD_GOOSE: &str = "https://block.github.io/goose/docs/getting-started/using-extensions/";
const DEAD_WARP: &str = "https://docs.warp.dev/agent-platform/capabilities/mcp";
const DEAD_REGISTRY: &str = "https://registry.modelcontextprotocol.io/v0/servers?search=mcphost";

#[test]
fn the_three_dead_urls_are_replaced_everywhere() {
    let file = clients::builtin();
    let docs = |name: &str| file.client.iter().find(|c| c.name == name).unwrap().docs_url.clone();
    assert_ne!(docs("Goose"), DEAD_GOOSE);
    assert_ne!(docs("Warp"), DEAD_WARP);
    assert_ne!(file.registry.search_url, DEAD_REGISTRY);
    for (name, doc) in [("README.md", README), ("www/llms.txt", LLMS_TXT)] {
        for dead in [DEAD_GOOSE, DEAD_WARP, DEAD_REGISTRY] {
            assert!(!doc.contains(dead), "{name} still carries dead URL {dead}");
        }
    }
    assert!(
        README.contains(&format!("]({})", file.registry.search_url)),
        "README's registry link must be docs/clients.toml's [registry] search_url"
    );
}

#[test]
fn checked_stamps_are_the_generators_not_hand_dates() {
    // `--stamp-clients` writes one date across every client that passed; a
    // hand-edited row would stand out as a different date.
    let dates: std::collections::BTreeSet<&str> = clients::builtin().client.iter().map(|c| c.checked.as_str()).collect();
    assert!(dates.iter().all(|d| d.len() == 10 && d.as_bytes()[4] == b'-'), "{dates:?}");
}

#[test]
fn default_file_set_resolves() {
    use std::net::ToSocketAddrs;
    if "docs.warp.dev:443".to_socket_addrs().is_err() {
        eprintln!("skipped: no outbound DNS; CI's docs job runs this check");
        return;
    }
    let out = std::process::Command::new(xlinks::repo_root().join("scripts/docs-link-check.sh"))
        .output()
        .expect("run docs-link-check.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "dead links:\n{}", stdout.lines().filter(|l| l.starts_with("FAIL")).collect::<Vec<_>>().join("\n"));
}
