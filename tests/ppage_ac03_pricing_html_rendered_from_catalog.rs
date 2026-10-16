//! PRD-mcphost-pricing-page-from-plan-table AC3 (P0) -- Given the committed
//! `www/pricing.html`, When `gendocs::render_pricing_html(default_catalog())`
//! runs, Then the output is byte-identical and every digit in it is
//! traceable to a slot (the template source contains no digit outside slot
//! names).

use mcphost::gendocs::{pricing_prose_parts, pricing_slots, pricing_template_sources, render_pricing_html};
use mcphost::plans::PlanCatalog;

/// Tag names `h1`/`h2` are markup, not prose numbers.
fn without_heading_tags(text: &str) -> String {
    let mut out = text.to_string();
    for tag in ["h1", "h2"] {
        out = out.replace(&format!("</{tag}"), "").replace(&format!("<{tag}"), "");
    }
    out
}

#[test]
fn committed_pricing_html_is_the_fresh_render() {
    let committed = include_str!("../www/pricing.html");
    let fresh = render_pricing_html(&PlanCatalog::default_catalog());
    assert_eq!(committed, fresh, "www/pricing.html has drifted -- run `mcphost gen-docs`");
}

#[test]
fn template_sources_carry_no_digit_outside_slot_names() {
    let catalog = PlanCatalog::default_catalog();
    for source in pricing_template_sources() {
        let mut text = without_heading_tags(source);
        for (name, _) in pricing_slots(&catalog) {
            text = text.replace(&format!("{{{name}}}"), "");
        }
        let at = text.find(|c: char| c.is_ascii_digit());
        assert!(at.is_none(), "digit outside a slot in the pricing template near: {:?}", at.map(|i| &text[i.saturating_sub(30)..(i + 30).min(text.len())]));
    }
}

#[test]
fn every_rendered_prose_digit_comes_from_a_slot_value() {
    let catalog = PlanCatalog::default_catalog();
    let slots = pricing_slots(&catalog);
    for part in pricing_prose_parts(&catalog) {
        let mut text = without_heading_tags(&part);
        // Longest values first so `50,000` goes before `50`.
        let mut values: Vec<&String> = slots.iter().map(|(_, v)| v).collect();
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        for value in values {
            text = text.replace(value.as_str(), "");
        }
        assert!(
            !text.chars().any(|c| c.is_ascii_digit()),
            "a digit in the rendered pricing prose is not a slot value: {text}"
        );
    }
}

#[test]
fn a_changed_catalog_price_changes_the_page() {
    let mut catalog = PlanCatalog::default_catalog();
    catalog.plans.iter_mut().find(|p| p.name == "pro").expect("pro").price_usd_month = 29;
    let page = render_pricing_html(&catalog);
    assert!(page.contains("$29 a month"), "{page}");
    assert!(!page.contains("$19"), "a stale $19 survived");
}
