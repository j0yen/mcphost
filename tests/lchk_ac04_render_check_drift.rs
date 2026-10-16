//! PRD-mcphost-launch-check-generated AC4 (P0) -- Given
//! `docs/launch/wave1-checklist.md` committed, When `launch-check --render
//! --check` runs after a row's `doc` is edited in the TOML without
//! re-rendering, Then it exits 1 naming the file; after `--render`, it exits
//! 0. Also R4: the rendered `readme-link-*` rows equal the README's link set.

use std::path::Path;
use std::process::{Command, Output};

const TOML: &str = r#"
[[item]]
id = "p-ok"
wave = 1
owner = "host"
probe = "echo ok"
expect = "ok"
doc = "prints ok"

[[item]]
id = "j-submit"
wave = 1
owner = "joe"
doc = "Click submit."
url = "https://example.com/submit"
"#;

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("launch-check")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[test]
fn lchk_ac04_stale_checklist_exits_one_naming_the_file_and_render_fixes_it() {
    let dir = std::env::temp_dir().join(format!("lchk-ac04-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("launch.toml"), TOML).unwrap();
    let render = run(&dir, &["--render", "--items", "launch.toml"]);
    assert!(render.status.success(), "{}", String::from_utf8_lossy(&render.stderr));
    let file = dir.join("docs/launch/wave1-checklist.md");
    assert!(file.exists());

    let ok = run(&dir, &["--render", "--check", "--items", "launch.toml"]);
    assert_eq!(ok.status.code(), Some(0));

    std::fs::write(dir.join("launch.toml"), TOML.replace("prints ok", "prints ok, edited")).unwrap();
    let stale = run(&dir, &["--render", "--check", "--items", "launch.toml"]);
    assert_eq!(stale.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("docs/launch/wave1-checklist.md"),
        "{}",
        String::from_utf8_lossy(&stale.stderr)
    );

    assert!(run(&dir, &["--render", "--items", "launch.toml"]).status.success());
    let fixed = run(&dir, &["--render", "--check", "--items", "launch.toml"]);
    assert_eq!(fixed.status.code(), Some(0));
}

#[test]
fn lchk_ac04_readme_links_render_one_row_per_external_host() {
    let dir = std::env::temp_dir().join(format!("lchk-ac04r-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("launch.toml"), TOML).unwrap();
    std::fs::write(
        dir.join("README.md"),
        "See [a](https://alpha.example.org/docs) and https://beta.example.net/x, plus http://localhost:3000/ and https://alpha.example.org/other.\n",
    )
    .unwrap();
    assert!(run(&dir, &["--render", "--items", "launch.toml"]).status.success());
    let md = std::fs::read_to_string(dir.join("docs/launch/wave1-checklist.md")).unwrap();
    assert!(md.contains("| readme-link-alpha-example-org |"), "{md}");
    assert!(md.contains("| readme-link-beta-example-net |"), "{md}");
    assert!(md.contains("https://alpha.example.org/other"), "{md}");
    assert_eq!(md.matches("| readme-link-").count(), 2, "{md}");
    assert!(!md.contains("localhost"), "{md}");
}
