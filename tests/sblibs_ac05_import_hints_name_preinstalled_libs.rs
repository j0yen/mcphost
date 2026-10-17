//! PRD-mcphost-sandbox-file-processing-libs
//! AC5 (P0) -- Given a tool `import PIL.Imag`, When published, Then the
//! refusal hint reads `PIL is preinstalled (Pillow <version>): import
//! PIL.Image`; Given `import pandas`, Then the hint names `requirements` and
//! lists the five preinstalled names.

use crate::common;
use common::{TestServer, python_kind_registry, signup};
use mcphost::sandbox_libs;
use serde_json::json;

async fn publish_refusal(label: &str, spec: serde_json::Value) -> (String, serde_json::Value) {
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (_ns, key) = signup(&server.base_url, &format!("Sblibs AC5 {label}")).await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);
    let err = client
        .tools_call("host.tool_publish", json!({"name": "typo_tool", "kind": "python", "spec": spec}))
        .await
        .expect_err("the import must be refused at publish");
    assert_eq!(err.data["error_code"], json!("unknown_import"), "{label}: {err:?}");
    (err.data["hint"].as_str().expect("data.hint is a string").to_string(), err.data.clone())
}

#[tokio::test]
async fn sblibs_ac05_pil_typo_names_the_preinstalled_pillow_and_the_fix() {
    let pillow = sandbox_libs::libs().iter().find(|l| l.name == "Pillow").expect("Pillow row");
    let (hint, data) = publish_refusal(
        "pil",
        json!({"source": "import PIL.Imag\ndef main(args):\n    return {}\n"}),
    )
    .await;
    assert_eq!(hint, format!("PIL is preinstalled (Pillow {}): import PIL.Image", pillow.version));
    assert_eq!(data["module"], json!("PIL.Imag"));
}

#[test]
fn sblibs_ac05_valid_listed_imports_get_no_typo_hint() {
    for valid in ["PIL", "PIL.Image", "pypdf", "openpyxl", "dateutil.parser", "chardet"] {
        assert!(sandbox_libs::typo_hint(valid).is_none(), "{valid} is valid, not a typo");
    }
    assert!(sandbox_libs::typo_hint("pypdf2").is_some_and(|h| h.contains("import pypdf")));
}

#[tokio::test]
async fn sblibs_ac05_pandas_beside_other_requirements_names_requirements_and_the_five_libs() {
    let (hint, _data) = publish_refusal(
        "pandas",
        json!({
            "source": "import pandas\ndef main(args):\n    return {}\n",
            "requirements": ["httpx"],
        }),
    )
    .await;
    assert!(hint.contains("requirements"), "hint names `requirements`: {hint}");
    for lib in sandbox_libs::libs() {
        assert!(hint.contains(&lib.name), "hint lists {}: {hint}", lib.name);
    }
}
