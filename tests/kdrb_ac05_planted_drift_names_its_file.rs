//! contract: reads scripts/generated-docs-check.sh
//! PRD-mcphost-kind-docs-from-registry-b-v2 AC5: one planted drift per
//! target the two retired scripts guarded -- the sandbox-API block in each of
//! its four files, a spec field missing from a `kind-fields` table, and each
//! of the eight targets the renamed umbrella keeps -- exits 1 naming its
//! file, from `mcphost gen-docs kinds --check` or `generated-docs-check.sh`
//! as appropriate; the untouched tree exits 0 from both.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::common;

fn kinds_check(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["gen-docs", "kinds", "--check"])
        .current_dir(dir)
        .output()
        .expect("run gen-docs kinds --check")
}

fn umbrella(dir: &Path) -> Output {
    Command::new("bash")
        .arg(dir.join("scripts/generated-docs-check.sh"))
        .env("MCPHOST_BIN", env!("CARGO_BIN_EXE_mcphost"))
        .output()
        .expect("run generated-docs-check.sh")
}

/// A scratch tree: the umbrella's inputs plus every kind page.
fn tree(label: &str) -> PathBuf {
    let dir = common::doc_check_scratch(label);
    let pages = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/kinds");
    for entry in std::fs::read_dir(pages).expect("read docs/kinds") {
        let entry = entry.expect("entry");
        std::fs::copy(entry.path(), dir.join("docs/kinds").join(entry.file_name())).expect("copy page");
    }
    dir
}

/// `text` with the first line starting with `prefix` removed.
fn drop_line(text: &str, prefix: &str) -> String {
    let mut done = false;
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| {
            let hit = !done && l.starts_with(prefix);
            done |= hit;
            !hit
        })
        .collect();
    assert!(done, "no line starts with {prefix}");
    kept.join("\n") + "\n"
}

fn replace_once(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "no `{from}` to plant over");
    text.replacen(from, to, 1)
}

enum Check {
    Kinds,
    Umbrella,
}

/// (label, file the drift lands in, the check that guards it, the edit).
#[allow(clippy::type_complexity)]
fn plants() -> Vec<(&'static str, &'static str, Check, Box<dyn Fn(&str) -> String>)> {
    let sandbox = |text: &str| drop_line(text, "- `mcphost.lineage` -- ");
    vec![
        ("sandbox-python", "docs/kinds/python.md", Check::Kinds, Box::new(sandbox)),
        ("sandbox-js", "docs/kinds/js.md", Check::Kinds, Box::new(sandbox)),
        ("sandbox-llms", "www/llms.txt", Check::Kinds, Box::new(sandbox)),
        ("sandbox-skill", "plugin/skills/mcphost/SKILL.md", Check::Kinds, Box::new(sandbox)),
        // a spec field missing from the table: the chain page's `steps` row.
        ("kind-fields", "docs/kinds/chain.md", Check::Kinds, Box::new(|t| drop_line(t, "| `steps` |"))),
        (
            "bridge-block",
            "docs/sandbox-api.md",
            Check::Umbrella,
            Box::new(|t| drop_line(t, "- `mcphost.state` -- ")),
        ),
        (
            "capabilities",
            "www/llms.txt",
            Check::Umbrella,
            Box::new(|t| replace_once(t, "<!-- capabilities:start -->\n", "<!-- capabilities:start -->\ndrift\n")),
        ),
        (
            "recipe",
            "README.md",
            Check::Umbrella,
            Box::new(|t| {
                replace_once(t, "<!-- quickstart-recipe:sub_agent_key -->\n", "<!-- quickstart-recipe:sub_agent_key -->\ndrift\n")
            }),
        ),
        ("prompts", "docs/protocol/prompts.md", Check::Umbrella, Box::new(|t| replace_once(t, "# MCP prompts", "# MCP prompts (edited)"))),
        ("frames", "docs/protocol/frames.md", Check::Umbrella, Box::new(|t| t.to_string() + "drift\n")),
        ("host-tools", "docs/host-tools.md", Check::Umbrella, Box::new(|t| drop_line(t, "| `"))),
        (
            "signup-args",
            "www/skill.md",
            Check::Umbrella,
            Box::new(|t| replace_once(t, "<!-- mcphost-signup-args:start -->\n", "<!-- mcphost-signup-args:start -->\ndrift\n")),
        ),
        (
            "oauth-entry",
            "docs/oauth-connect.md",
            Check::Umbrella,
            Box::new(|t| t.replace("https://mcphost.dev/login/mcp", "https://mcphost.dev/login/elsewhere")),
        ),
    ]
}

#[test]
fn kdrb_ac05() {
    let clean = tree("kdrb-ac05-clean");
    for (name, out) in [("gen-docs kinds --check", kinds_check(&clean)), ("generated-docs-check.sh", umbrella(&clean))] {
        assert!(out.status.success(), "{name} on the untouched tree: {}", String::from_utf8_lossy(&out.stderr));
    }
    std::fs::remove_dir_all(&clean).ok();

    for (label, file, check, edit) in plants() {
        let dir = tree(&format!("kdrb-ac05-{label}"));
        let path = dir.join(file);
        let before = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
        let after = edit(&before);
        assert_ne!(before, after, "{label}: the plant must change {file}");
        std::fs::write(&path, after).expect("plant the drift");
        let out = match check {
            Check::Kinds => kinds_check(&dir),
            Check::Umbrella => umbrella(&dir),
        };
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{label}: a drift in {file} must exit 1, stderr:\n{stderr}");
        assert!(stderr.contains(file), "{label}: stderr must name {file}:\n{stderr}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
