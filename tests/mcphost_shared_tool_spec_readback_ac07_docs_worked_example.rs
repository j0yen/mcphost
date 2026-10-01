//! PRD-mcphost-shared-tool-spec-readback
//! AC7 (P0) — Given `docs/agent-quickstart.md` and `www/llms.txt` after
//! this PRD, When they are grepped, Then both contain `expose_spec` and
//! `host.tool.spec_shared` with a worked example.
// PRD-mcphost-tool-naming-convention-and-aliases: updated to the canonical name -- docs/www/llms.txt now read host.<family>.<verb>, not the old underscore form this test used to parse/compare against.

use std::fs;
use std::path::Path;

const QUICKSTART: &str = "docs/agent-quickstart.md";
const LLMS_TXT: &str = "www/llms.txt";

fn read(rel: &str) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| panic!("{rel} must exist and be readable: {e}"))
}

#[test]
fn both_docs_name_expose_spec_and_tool_spec_shared_with_a_worked_example() {
    for (path, text) in [(QUICKSTART, read(QUICKSTART)), (LLMS_TXT, read(LLMS_TXT))] {
        assert!(text.contains("expose_spec"), "{path} must mention expose_spec");
        assert!(
            text.contains("host.tool.spec_shared"),
            "{path} must mention host.tool.spec_shared"
        );
        // A worked example: an actual `host.tool.spec_shared(tool=...)` call,
        // not just a mention of the name in prose.
        assert!(
            text.contains("host.tool.spec_shared(tool="),
            "{path} must show a worked host.tool.spec_shared(tool=...) call"
        );
        // The redaction guarantee, restated beside the example.
        assert!(
            text.contains("spec_not_exposed"),
            "{path} must name the spec_not_exposed refusal"
        );
    }
}
