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

/// Runs the real checker over `file` with a temp allowlist and a temp
/// known-good list (hosts, one per line) and a 1 s timeout; no internet.
fn run_fixture(file: &std::path::Path, known_good: &str, extra: &[&str]) -> (std::process::Output, std::path::PathBuf) {
    let dir = file.parent().unwrap();
    let (allow, good) = (dir.join("allow.txt"), dir.join("known-good.txt"));
    std::fs::write(&allow, "# no allowlisted URLs in this fixture\n").unwrap();
    std::fs::write(&good, known_good).unwrap();
    let out = std::process::Command::new(xlinks::repo_root().join("scripts/docs-link-check.sh"))
        .args(extra)
        .arg(file)
        .env("DOCS_LINK_TIMEOUT", "1")
        .env("DOCS_LINK_ALLOW", &allow)
        .env("DOCS_LINK_KNOWN_GOOD", &good)
        .output()
        .expect("run docs-link-check.sh");
    (out, good)
}

/// Replaces the former whole-docs-set run, which made live HTTP calls from
/// the core suites and failed a required check on an external host's
/// latency. The live whole-set check is CI's `docs` job (non-blocking on
/// timeouts); this proves the resolver's three verdicts on a fixture.
#[test]
fn default_file_set_resolves() {
    let port = xlinks::serve();
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port() // dropped: nothing listens here
    };
    let dir = xlinks::scratch("ac06");
    let file = dir.join("doc.md");
    let cached = "http://cached-host.invalid/page";
    let unreachable = format!("http://127.0.0.1:{closed}/x");
    let gone = format!("http://127.0.0.1:{port}/gone");

    // (a) known-good host -> `OK cached`, never requested; (b) unreachable ->
    // WARN, exit 0.
    std::fs::write(&file, format!("[a]({cached})\n[b]({unreachable})\n")).unwrap();
    let (out, _) = run_fixture(&file, "# refreshed 2026-10-15\ncached-host.invalid\n", &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    assert!(stdout.contains(&format!("OK cached {cached} {}:1", file.display())), "{stdout}");
    assert!(stdout.contains(&format!("WARN error {unreachable} {}:2", file.display())), "{stdout}");
    assert!(!stdout.contains("FAIL"), "{stdout}");

    // (c) a definitive 404 -> FAIL, exit 1, even next to a WARN.
    std::fs::write(&file, format!("[b]({unreachable})\n[c]({gone})\n")).unwrap();
    let (out, _) = run_fixture(&file, "", &[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "stdout: {stdout}");
    assert!(stdout.contains(&format!("FAIL 404 {gone} {}:2", file.display())), "{stdout}");
    assert!(stdout.contains(&format!("WARN error {unreachable} {}:1", file.display())), "{stdout}");
}

#[test]
fn refresh_keeps_resolving_hosts_and_drops_dead_and_unreferenced_ones() {
    let port = xlinks::serve();
    let dir = xlinks::scratch("ac06r");
    let file = dir.join("doc.md");
    std::fs::write(&file, format!("[a](http://127.0.0.1:{port}/ok)\n")).unwrap();
    let (out, good) = run_fixture(&file, "127.0.0.1\nstale-host.invalid\n", &["--refresh-known-good"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    assert!(stdout.contains("KEEP 127.0.0.1") && stdout.contains("DROP stale-host.invalid unreferenced"), "{stdout}");
    let text = std::fs::read_to_string(&good).unwrap();
    assert!(text.starts_with("# refreshed 20"), "{text}");
    let hosts: Vec<&str> = text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(hosts, vec!["127.0.0.1"], "{text}");

    // The same host now carries a dead link -> dropped, so the cache can
    // never vouch for a host with a 404 on it.
    std::fs::write(&file, format!("[a](http://127.0.0.1:{port}/ok)\n[b](http://127.0.0.1:{port}/gone)\n")).unwrap();
    let (out, good) = run_fixture(&file, "127.0.0.1\n", &["--refresh-known-good"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout: {stdout}");
    assert!(stdout.contains("DROP 127.0.0.1 dead-link 404"), "{stdout}");
    let text = std::fs::read_to_string(&good).unwrap();
    assert!(text.lines().all(|l| l.starts_with('#') || l.is_empty()), "{text}");
}
