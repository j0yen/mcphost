//! PRD-mcphost-sandbox-file-processing-libs
//! AC4 (P0) -- Given a python tool `import pypdf` with no `requirements`,
//! When published and called under `network: none`, Then publish reports
//! `env: preinstalled, build_ms: 0` and the call extracts text from the
//! 3-page fixture PDF.

use crate::common;
use base64::Engine as _;
use common::{TestServer, extract_structured, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::json;

const SOURCE: &str = "import base64, io\nimport pypdf\n\ndef main(args):\n    reader = pypdf.PdfReader(io.BytesIO(base64.b64decode(args['pdf_b64'])))\n    return {'pages': len(reader.pages), 'text': ' | '.join(p.extract_text().strip() for p in reader.pages)}\n";

#[tokio::test]
async fn sblibs_ac04_publish_reports_preinstalled_and_call_reads_the_pdf() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Sblibs AC4 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": SOURCE,
        "network": "none",
        "args_schema": {"type": "object", "properties": {"pdf_b64": {"type": "string"}}, "required": ["pdf_b64"]},
    });
    let published = extract_structured(
        &client
            .tools_call("host.tool_publish", json!({"name": "pdf_text", "kind": "python", "spec": spec}))
            .await
            .expect("publish ok"),
    );
    assert_eq!(published["env"], json!("preinstalled"), "publish response: {published}");
    assert_eq!(published["build_ms"], json!(0), "publish response: {published}");

    let pdf = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sblibs/three_pages.pdf"))
        .expect("fixture pdf");
    let pdf_b64 = base64::engine::general_purpose::STANDARD.encode(pdf);

    let mut last = json!(null);
    for _ in 0..200 {
        let result = client
            .tools_call(&format!("{ns}.pdf_text"), json!({"pdf_b64": pdf_b64}))
            .await
            .unwrap_or_else(|e| panic!("call failed: {} {}", e.code, e.message));
        last = extract_structured(&result);
        if last["status"] != json!("building") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(last["pages"], json!(3), "call result: {last}");
    let text = last["text"].as_str().expect("text");
    for expected in ["Alpha page one", "Bravo page two", "Charlie page three"] {
        assert!(text.contains(expected), "extracted text must contain {expected:?}: {text}");
    }
}
