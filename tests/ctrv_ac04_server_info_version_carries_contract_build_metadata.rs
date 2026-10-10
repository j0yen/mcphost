//! PRD-mcphost-contract-version-reported AC4 — Given an `initialize`
//! response, When `serverInfo.version` is parsed by the `semver` crate,
//! Then `build` metadata equals `contract.1.<sha12>` and `serverInfo.name`
//! is `mcphost`.

use crate::common;
use common::{McpClient, TestServer};

#[tokio::test]
async fn server_info_version_has_contract_build_metadata() {
    let server = TestServer::start().await;
    let init = McpClient::new(&server.base_url).initialize().await;
    let info = init
        .get("result")
        .unwrap_or(&init)
        .get("serverInfo")
        .unwrap_or_else(|| panic!("initialize must carry serverInfo: {init}"));
    assert_eq!(info["name"], "mcphost", "{info}");
    let version = info["version"].as_str().expect("serverInfo.version string");
    let parsed = semver::Version::parse(version)
        .unwrap_or_else(|e| panic!("serverInfo.version {version:?} must be valid semver: {e}"));
    assert_eq!(
        parsed.build.as_str(),
        format!("contract.1.{}", &server.state.contract.sha[..12]),
        "{version}"
    );
    assert_eq!(parsed.to_string(), version);
    assert_eq!(
        format!("{}.{}.{}", parsed.major, parsed.minor, parsed.patch),
        env!("CARGO_PKG_VERSION")
    );
}
