//! PRD-mcphost-unknown-kind-routes-to-recipe
//! AC6 (P0) -- Given `contracts/host-tools.v1.json` and `www/llms.txt` at
//! the landing commit, When grepped, Then the `kind` description and the
//! kinds paragraph both contain `webhook` and `alias`.

#[test]
fn contract_kind_description_mentions_webhook_and_alias() {
    let contract = std::fs::read_to_string("contracts/host-tools.v1.json")
        .expect("read contracts/host-tools.v1.json");
    assert!(
        contract.contains("webhook"),
        "contracts/host-tools.v1.json must mention webhook"
    );
    assert!(
        contract.contains("alias"),
        "contracts/host-tools.v1.json must mention alias"
    );
}

#[test]
fn llms_txt_kinds_paragraph_mentions_webhook_and_alias() {
    let llms = std::fs::read_to_string("www/llms.txt").expect("read www/llms.txt");
    assert!(llms.contains("webhook"), "www/llms.txt must mention webhook");
    assert!(llms.contains("alias"), "www/llms.txt must mention alias");
}
