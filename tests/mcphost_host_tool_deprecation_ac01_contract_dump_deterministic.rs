//! PRD-mcphost-host-tool-deprecation AC1 — Given the current registry, When
//! `contract dump` runs twice, Then the outputs are byte-identical and
//! match the committed v1 file.

use mcphost::api_contract::dump_contract_bytes;
use mcphost::kinds::KindRegistry;

/// Compiled in, like `surface_ac05_llms_txt_tool_parity.rs`'s own
/// `include_str!` of `www/llms.txt` -- proves the committed file, not
/// whatever happens to be on disk relative to the test binary's cwd.
const COMMITTED_CONTRACT: &[u8] = include_bytes!("../contracts/host-tools.v1.json");

#[test]
fn contract_dump_is_byte_identical_across_runs_and_matches_the_committed_file() {
    let kinds = KindRegistry::with_builtin();
    let first = dump_contract_bytes(&kinds);
    let second = dump_contract_bytes(&kinds);
    assert_eq!(
        first, second,
        "two `contract dump` runs against the same registry must be byte-identical"
    );
    assert_eq!(
        first, COMMITTED_CONTRACT,
        "contracts/host-tools.v1.json is stale -- regenerate with `mcphost contract dump`"
    );
}
