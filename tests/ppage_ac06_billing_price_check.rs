//! PRD-mcphost-pricing-page-from-plan-table AC6 (P1) -- Given a fake billing
//! client whose pro price tier says 40 000 included, When `mcphost billing
//! price-check` runs, Then it prints both numbers, names the field, and
//! exits 1; Given tiers matching the catalog, Then it exits 0.

use mcphost::billing::{BillingConfig, FakeBillingClient, StripePrice, StripePriceTier, price_check};
use mcphost::plans::PlanCatalog;

fn config() -> BillingConfig {
    BillingConfig {
        secret_key: Some("sk_test_fake".into()),
        price_pro: Some("price_base".into()),
        metered_price_id: Some("price_metered".into()),
        ..BillingConfig::default()
    }
}

fn fake(included: i64) -> FakeBillingClient {
    let fake = FakeBillingClient::new(0);
    fake.set_price("price_base", StripePrice { unit_amount: Some(1900), tiers: vec![] });
    fake.set_price(
        "price_metered",
        StripePrice {
            unit_amount: None,
            tiers: vec![
                StripePriceTier { up_to: Some(included), unit_amount_cents: 0.0 },
                StripePriceTier { up_to: None, unit_amount_cents: 0.1 },
            ],
        },
    );
    fake
}

#[tokio::test]
async fn forty_thousand_included_prints_both_numbers_names_the_field_and_exits_1() {
    let report = price_check(&fake(40_000), &config(), &PlanCatalog::default_catalog())
        .await
        .expect("price-check runs");
    let out = report.lines.join("\n");
    assert!(out.contains("50000 included"), "catalog number missing: {out}");
    assert!(out.contains("40000 included"), "stripe number missing: {out}");
    assert!(out.contains("calls_included_month"), "field not named: {out}");
    assert_eq!(report.exit_code(), 1);
}

#[tokio::test]
async fn matching_tiers_exit_0() {
    let report = price_check(&fake(50_000), &config(), &PlanCatalog::default_catalog())
        .await
        .expect("price-check runs");
    assert!(report.mismatches.is_empty(), "{:?}", report.lines);
    assert!(report.lines.join("\n").contains("100 c/1000"));
    assert_eq!(report.exit_code(), 0);
}

#[tokio::test]
async fn a_wrong_overage_rate_names_its_field() {
    let fake = fake(50_000);
    fake.set_price(
        "price_metered",
        StripePrice {
            unit_amount: None,
            tiers: vec![
                StripePriceTier { up_to: Some(50_000), unit_amount_cents: 0.0 },
                StripePriceTier { up_to: None, unit_amount_cents: 0.2 },
            ],
        },
    );
    let report = price_check(&fake, &config(), &PlanCatalog::default_catalog()).await.expect("runs");
    assert!(report.mismatches.iter().any(|m| m.starts_with("overage_usd_per_1000_calls")), "{:?}", report.mismatches);
    assert_eq!(report.exit_code(), 1);
}

#[test]
fn billing_mode_off_prints_skipped_and_exits_0() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["billing", "price-check"])
        .env_remove("MCPHOST_STRIPE_SECRET_KEY")
        .output()
        .expect("run mcphost");
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("skipped"));
}
