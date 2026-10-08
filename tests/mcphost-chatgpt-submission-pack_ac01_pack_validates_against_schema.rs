//! AC1 (PRD-mcphost-chatgpt-submission-pack) — Given the binary, When
//! `mcphost submission-pack --chatgpt out/` runs, Then `out/metadata.json`
//! validates against `docs/chatgpt-submission.schema.json` and
//! `out/icon-64.png` is 64×64 and under 5,120 bytes.

use std::process::Command;

#[test]
fn submission_pack_metadata_validates_and_icon_is_64px_under_5kb() {
    let out = std::env::temp_dir().join(format!("mcphost-pack-ac01-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let status = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["submission-pack", "--chatgpt"])
        .arg(&out)
        .env_remove("MCPHOST_PUBLIC_URL")
        .status()
        .expect("run mcphost submission-pack");
    assert!(status.success(), "submission-pack exits 0");

    let schema: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/chatgpt-submission.schema.json"))
            .expect("read schema"),
    )
    .expect("schema is json");
    let metadata: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("metadata.json")).expect("metadata.json")).unwrap();
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    let errors: Vec<String> = validator.iter_errors(&metadata).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "metadata.json violates the schema: {errors:?}");

    assert!(metadata["name"].as_str().unwrap().chars().count() <= 30);
    assert!(metadata["short_description"].as_str().unwrap().chars().count() <= 160);
    for key in ["website_url", "privacy_url", "terms_url", "mcp_url"] {
        assert!(metadata[key].as_str().unwrap().starts_with("https://"), "{key}: {metadata}");
    }

    // The schema must actually bite: an over-long name and an http URL fail.
    let mut bad = metadata.clone();
    bad["name"] = serde_json::json!("x".repeat(31));
    bad["privacy_url"] = serde_json::json!("http://mcphost.dev/privacy");
    assert!(!validator.is_valid(&bad), "schema accepted a 31-char name and an http URL");

    let icon = std::fs::read(out.join("icon-64.png")).expect("icon-64.png");
    assert_eq!(&icon[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A], "PNG signature");
    assert_eq!(&icon[12..16], b"IHDR");
    let width = u32::from_be_bytes(icon[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(icon[20..24].try_into().unwrap());
    assert_eq!((width, height), (64, 64));
    assert!(icon.len() < 5_120, "icon is {} bytes", icon.len());
    let _ = std::fs::remove_dir_all(&out);
}
