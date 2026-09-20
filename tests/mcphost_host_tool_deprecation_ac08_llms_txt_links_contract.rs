//! PRD-mcphost-host-tool-deprecation AC8 — Given `llms.txt`, When fetched,
//! Then it links the contract file.

const LLMS_TXT: &str = include_str!("../www/llms.txt");

#[test]
fn llms_txt_links_the_committed_contract_file() {
    assert!(
        LLMS_TXT.contains("contracts/host-tools.v1.json"),
        "www/llms.txt must link the current contract file"
    );
    assert!(
        LLMS_TXT.contains("host.changelog"),
        "www/llms.txt must point agents at host.changelog for what changed"
    );
}
