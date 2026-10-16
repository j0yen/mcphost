//! PRD-mcphost-pricing-page-from-plan-table AC4 (P0) -- Given
//! `tests/www_pages.sh`, When the catalog's pro price is changed to 29 in a
//! test fixture, Then the generated page says `$29` and the script's pricing
//! checks pass against `/plans.json`, with no literal `$19` in the script.

use crate::common::TestServer;
use mcphost::gendocs::render_pricing_html;
use std::process::Command;

fn pro_29(catalog: &mut mcphost::plans::PlanCatalog) {
    catalog.plans.iter_mut().find(|p| p.name == "pro").expect("pro").price_usd_month = 29;
}

#[test]
fn script_carries_no_literal_pro_price() {
    let script = include_str!("www_pages.sh");
    assert!(!script.contains("$19"), "tests/www_pages.sh must derive the price from /plans.json");
    assert!(!script.contains("19"), "no literal 19 in the script");
}

#[tokio::test]
async fn pricing_checks_pass_against_plans_json_with_a_changed_price() {
    let server = TestServer::start_with_plans(pro_29).await;
    let mut catalog = mcphost::plans::PlanCatalog::default_catalog();
    pro_29(&mut catalog);
    let page = render_pricing_html(&catalog);
    assert!(page.contains("$29"), "generated page must say $29");

    let dir = std::env::temp_dir().join(format!("ppage-ac04-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let page_path = dir.join("pricing.html");
    std::fs::write(&page_path, &page).expect("write page");

    let root = env!("CARGO_MANIFEST_DIR");
    let base = server.base_url.clone();
    let out = tokio::task::spawn_blocking(move || {
        Command::new("bash")
            .arg("tests/www_pages.sh")
            .current_dir(root)
            .env("WWW_PAGES_BASE_URL", base)
            .env("WWW_PAGES_PRICING", page_path)
            .output()
            .expect("run www_pages.sh")
    })
    .await
    .expect("join");
    std::fs::remove_dir_all(&dir).ok();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ok - pricing page names '$29' (from /plans.json)"), "{stdout}");
    for line in stdout.lines().filter(|l| l.contains("pricing page names")) {
        assert!(line.starts_with("ok - "), "pricing check failed: {line}\n{stdout}");
    }
    assert!(out.status.success(), "www_pages.sh failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
}
