//! PRD-mcphost-docs-import-and-stated-boundary
//! AC1 -- Given `tools/list` on the test host, When the `host.docs.put` and
//! `host.docs.put_many` descriptions, the zero-document `host.quickstart`
//! text and `examples/docs-qa/README.md` are read, Then each begins with the
//! R3 boundary sentence byte-identical to the one constant the test imports.
//! The "Docs Q&A in a minute" section of `www/llms.txt` (R3) leads with it too.

use crate::common;
use crate::dimp;

use common::{TestServer, extract_structured};
use mcphost::docs::DOCS_BOUNDARY;
use serde_json::json;

const README: &str = include_str!("../examples/docs-qa/README.md");
const LLMS_TXT: &str = include_str!("../www/llms.txt");

fn description_of(tools: &serde_json::Value, name: &str) -> String {
    tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("tools/list must carry {name}"))["description"]
        .as_str()
        .expect("description")
        .to_string()
}

#[tokio::test]
async fn dimp_ac01_every_surface_leads_with_the_boundary_sentence() {
    assert!(
        DOCS_BOUNDARY.starts_with("Documents are text the host indexes itself;")
            && DOCS_BOUNDARY.contains("LlamaIndex, Chroma, pgvector")
            && DOCS_BOUNDARY.ends_with("export its source documents and put them."),
        "the constant is the R3 sentence: {DOCS_BOUNDARY}"
    );

    let server = TestServer::start().await;
    let client = dimp::tenant(&server, "dimp ac1").await;

    let tools = client.tools_list().await.expect("tools/list");
    for name in ["host.docs.put", "host.docs.put_many"] {
        let description = description_of(&tools, name);
        assert!(description.starts_with(DOCS_BOUNDARY), "{name} description must begin with the boundary: {description}");
    }

    // A tenant with zero documents.
    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    let text = quickstart["docs"]["text"].as_str().expect("zero-document quickstart carries docs.text");
    assert!(text.starts_with(DOCS_BOUNDARY), "quickstart docs text must begin with the boundary: {text}");

    // Once it holds a document the hint is gone.
    client
        .tools_call("host.docs.put", json!({"name": "a.md", "content": "# a"}))
        .await
        .expect("put");
    let later = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.expect("quickstart"));
    assert!(later.get("docs").is_none(), "a tenant with documents gets no zero-document text: {later}");

    assert!(README.starts_with(DOCS_BOUNDARY), "examples/docs-qa/README.md must begin with the boundary");

    let heading = "## Docs Q&A in a minute\n\n";
    let at = LLMS_TXT.find(heading).expect("llms.txt has the section") + heading.len();
    assert!(LLMS_TXT[at..].starts_with(DOCS_BOUNDARY), "the llms.txt section must begin with the boundary, before the first command");

}
