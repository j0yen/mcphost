//! AC5 (PRD-mcphost-llms-install-doc) — Given `www/sitemap.xml`, When
//! fetched, Then it lists `/llms-install.md`, and `mcphost-deploy
//! vendor-www` succeeds with the new page.
//!
//! `vendor-www` refuses a page whose route the sitemap does not name; the
//! second test re-runs that exact rule (`.html` pages -> `/<stem>`,
//! `index.html` -> `/`, everything else under its own filename) over every
//! file in `www/` except the deploy-owned ones.

use crate::common;
use common::TestServer;

const SITEMAP: &str = include_str!("../www/sitemap.xml");

#[tokio::test]
async fn sitemap_is_served_and_lists_the_install_doc() {
    let server = TestServer::start().await;
    let response = reqwest::get(format!("{}/sitemap.xml", server.base_url)).await.expect("GET /sitemap.xml");
    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("body");
    assert_eq!(body, SITEMAP);
    assert!(body.contains("<loc>https://mcphost.dev/llms-install.md</loc>"), "{body}");
}

#[test]
fn every_vendored_www_page_has_a_sitemap_route() {
    let www = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("www");
    let mut checked = Vec::new();
    for entry in std::fs::read_dir(www).expect("www/") {
        let name = entry.expect("entry").file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "robots.txt" || name == "sitemap.xml" {
            continue;
        }
        let route = match name.strip_suffix(".html") {
            Some("index") => "/".to_string(),
            Some(stem) => format!("/{stem}"),
            None => format!("/{name}"),
        };
        let loc = format!("<loc>https://mcphost.dev{route}</loc>");
        assert!(SITEMAP.contains(&loc), "vendor-www would refuse {name}: sitemap lacks {loc}");
        checked.push(name);
    }
    assert!(checked.contains(&"llms-install.md".to_string()), "www/llms-install.md must exist: {checked:?}");
}

#[test]
fn www_copy_is_the_committed_install_doc() {
    assert_eq!(include_str!("../www/llms-install.md"), include_str!("../llms-install.md"));
}

/// Drives the REAL `mcphost_deploy.www_vendor.vendor_www` (not a mirror of its
/// rule) against this checkout's committed `www/` into a scratch dest holding
/// only this tree's sitemap.xml; the journal/state writes are stubbed so the
/// installed tool is untouched. Skips (loudly) where the deploy tool is not
/// installed, e.g. the runner box.
#[test]
fn real_vendor_www_accepts_the_new_page() {
    let home = std::env::var("HOME").unwrap_or_default();
    let python = std::env::var("MCPHOST_DEPLOY_PYTHON")
        .unwrap_or_else(|_| format!("{home}/.local/share/uv/tools/mcphost-deploy/bin/python"));
    if !std::path::Path::new(&python).is_file() {
        eprintln!("SKIP real_vendor_www_accepts_the_new_page: no deploy-tool python at {python}");
        return;
    }
    let repo = env!("CARGO_MANIFEST_DIR");
    if !std::process::Command::new("git").args(["-C", repo, "rev-parse", "HEAD"]).output().map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("SKIP real_vendor_www_accepts_the_new_page: not a git checkout");
        return;
    }
    let dest = std::env::temp_dir().join(format!("vendor-www-ac05-{}", std::process::id()));
    std::fs::create_dir_all(&dest).expect("mkdir");
    std::fs::write(dest.join("sitemap.xml"), SITEMAP).expect("seed sitemap");
    let script = "import sys\nfrom pathlib import Path\nfrom unittest import mock\n\
from mcphost_deploy import www_vendor as w\n\
with mock.patch.object(w,'_append_journal'), mock.patch.object(w,'_write_state'):\n\
\x20   r = w.vendor_www(from_path=Path(sys.argv[1]), sha='HEAD', dest_dir=Path(sys.argv[2]))\n\
print(','.join(r.written))\n";
    let out = std::process::Command::new(&python)
        .args(["-I", "-c", script, repo, dest.to_str().unwrap()])
        .output()
        .expect("run python");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let _ = std::fs::remove_dir_all(&dest);
    assert!(out.status.success(), "vendor_www refused: {stderr}");
    assert!(stdout.split(',').any(|n| n.trim() == "llms-install.md"), "llms-install.md not vendored: {stdout}");
}
