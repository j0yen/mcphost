//! PRD-mcphost-llms-full-generated-from-schemas AC5: a paragraph mentioning
//! `host.tool_share` outside its section and absent from
//! `docs/llms-full.prose.toml` fails `mcphost llms-full --check` (exit 1,
//! naming file and line); after the entry is added it exits 0.

use std::path::Path;
use std::process::{Command, Output};

fn mcphost(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("run mcphost")
}

#[test]
fn llf_ac05() {
    let root = std::env::temp_dir().join(format!("llf_ac05_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::create_dir_all(root.join("www")).unwrap();
    std::fs::write(root.join("www/llms-full.txt"), "# full\n").unwrap();
    std::fs::write(root.join("docs/llms-full.prose.toml"), "").unwrap();
    std::fs::write(
        root.join("docs/sharing-notes.md"),
        "# Sharing\n\nIntro.\n\n## Sharing a tool\n\nCall host.tool_share to hand a tool to a peer.\n",
    )
    .unwrap();

    let gen_out = mcphost(&root, &["llms-full"]);
    assert!(gen_out.status.success(), "{}", String::from_utf8_lossy(&gen_out.stderr));

    let bad = mcphost(&root, &["llms-full", "--check"]);
    assert_eq!(bad.status.code(), Some(1), "{}", String::from_utf8_lossy(&bad.stderr));
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(stderr.contains("docs/sharing-notes.md:7"), "{stderr}");
    assert!(stderr.contains("host.tool_share"), "{stderr}");

    std::fs::write(
        root.join("docs/llms-full.prose.toml"),
        "[[section]]\ntool = \"host.tool_share\"\npath = \"docs/sharing-notes.md#sharing-a-tool\"\n",
    )
    .unwrap();
    let regen = mcphost(&root, &["llms-full"]);
    assert!(regen.status.success(), "{}", String::from_utf8_lossy(&regen.stderr));
    let ok = mcphost(&root, &["llms-full", "--check"]);
    assert_eq!(ok.status.code(), Some(0), "{}", String::from_utf8_lossy(&ok.stderr));
    let text = std::fs::read_to_string(root.join("www/llms-full.txt")).unwrap();
    assert!(text.contains("Call host.tool_share to hand a tool to a peer."), "prose is inlined");

    let _ = std::fs::remove_dir_all(&root);
}
