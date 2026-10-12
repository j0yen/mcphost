//! AC3 (PRD-mcphost-call-cli-from-contract-b) -- Given `mcphost call
//! --completions zsh`, When sourced in zsh, Then `mcphost call host.r<TAB>`
//! lists `host.redeem`, `host.registry.publish`, `host.runs.*`.
//!
//! The script is sourced in a real interactive zsh with compinit on a pty and
//! `mcphost call host.r<TAB>` is typed; the test reads the listing zsh prints.
//! Where zsh is not installed the distro packages are unpacked under the
//! test's tmp dir (no root); with no zsh at all the test fails, never skips.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use mcphost::call_cli;

fn mcphost(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .arg("call")
        .args(args)
        .env_remove("MCPHOST_KEY")
        .output()
        .expect("run mcphost call")
}

fn zsh_candidates(script: &str) -> Vec<String> {
    let body = script.split("tools=(\n").nth(1).expect("tools=( array").split("\n  )").next().unwrap();
    body.lines()
        .map(|l| l.trim().trim_start_matches('\'').split(':').next().unwrap().to_string())
        .collect()
}

#[test]
fn tabbing_host_r_lists_redeem_registry_publish_and_the_runs_family() {
    let out = mcphost(&["--completions", "zsh"]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let script = String::from_utf8(out.stdout).expect("utf8");
    assert!(script.starts_with("#compdef mcphost"), "a zsh completion file");
    assert!(script.contains("compdef _mcphost_call mcphost"));

    let candidates = zsh_candidates(&script);
    let offered: Vec<&String> = candidates.iter().filter(|c| c.starts_with("host.r")).collect();
    println!("host.r<TAB> -> {offered:?}");
    assert!(offered.iter().any(|c| *c == "host.redeem"), "{offered:?}");
    assert!(offered.iter().any(|c| *c == "host.registry.publish"), "{offered:?}");
    assert!(offered.iter().any(|c| c.starts_with("host.runs.")), "{offered:?}");

    // Rendered from the table, not beside it: the candidates are exactly the
    // canonical tool names.
    let names: BTreeSet<&str> = call_cli::contract().tools.iter().map(|t| t.name.as_str()).collect();
    let got: BTreeSet<&str> = candidates.iter().map(String::as_str).collect();
    assert_eq!(got, names);

    // The tool's own flags complete once the tool is chosen.
    assert!(script.contains("host.tool.publish) _arguments"), "per-tool flag specs");
    assert!(script.contains("'--kind=["), "--kind completes");

    // Then source it in a real interactive zsh with the real completion
    // system (compinit, `_describe`), type `mcphost call host.r<TAB>` on a
    // pty, and read the listing zsh prints.
    let offered = zsh_tab(&script, "mcphost call host.r");
    println!("zsh host.r<TAB> -> {offered:?}");
    assert!(offered.contains("host.redeem"), "{offered:?}");
    assert!(offered.contains("host.registry.publish"), "{offered:?}");
    assert!(offered.iter().any(|c| c.starts_with("host.runs.")), "{offered:?}");
    let expected: BTreeSet<String> = names.iter().filter(|n| n.starts_with("host.r")).map(|n| n.to_string()).collect();
    assert_eq!(offered, expected, "zsh lists exactly the host.r* tools of the contract");
}

/// Drop terminal escape sequences (`ESC [ ... final-byte`) from pty output.
fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() || c == '~' {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A zsh binary plus, when it was unpacked rather than installed, the root
/// its modules and completion functions live under.
struct Zsh {
    bin: PathBuf,
    root: Option<PathBuf>,
}

/// The installed zsh, else the distro packages unpacked (no root) under the
/// test tmp dir. With neither the test fails: AC3 names zsh, so it never skips.
fn zsh_runtime() -> Zsh {
    let installed = Command::new("zsh").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if installed {
        return Zsh { bin: PathBuf::from("zsh"), root: None };
    }
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("zsh-pkg");
    let root = base.join("root");
    // The binary sits at usr/bin/zsh on a merged-/usr package (RedBaron's
    // 8ubuntu3) and at bin/zsh on noble's 6ubuntu2 (the GitHub runner).
    let zsh_in = |root: &Path| ["usr/bin/zsh", "bin/zsh"].iter().map(|p| root.join(p)).find(|p| p.exists());
    if zsh_in(&root).is_none() {
        std::fs::create_dir_all(&base).unwrap();
        let get = Command::new("apt-get")
            .current_dir(&base)
            .args(["download", "zsh", "zsh-common"])
            .output()
            .expect("run apt-get");
        assert!(get.status.success(), "no zsh installed and apt-get download failed: {}", String::from_utf8_lossy(&get.stderr));
        for entry in std::fs::read_dir(&base).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "deb") {
                let x = Command::new("dpkg-deb").arg("-x").arg(&path).arg(&root).output().expect("run dpkg-deb");
                assert!(x.status.success(), "dpkg-deb: {}", String::from_utf8_lossy(&x.stderr));
            }
        }
    }
    let bin = zsh_in(&root).unwrap_or_else(|| {
        panic!(
            "zsh unpacked (debs: {:?}; root entries: {:?})",
            std::fs::read_dir(&base).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name()).collect::<Vec<_>>()).unwrap_or_default(),
            std::fs::read_dir(&root).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name()).collect::<Vec<_>>()).unwrap_or_default()
        )
    });
    Zsh { bin, root: Some(root) }
}

/// Source `script` in an interactive zsh (compinit loaded) on a pty, type
/// `line` followed by TAB, and return the candidate names zsh lists.
fn zsh_tab(script: &str, line: &str) -> BTreeSet<String> {
    let z = zsh_runtime();
    let zsh = z.bin.clone();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cclib_ac03_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mcphost.zsh");
    std::fs::write(&file, script).unwrap();
    let check = Command::new(&zsh).arg("-n").arg(&file).output().unwrap();
    assert!(check.status.success(), "zsh -n: {}", String::from_utf8_lossy(&check.stderr));

    // An unpacked zsh looks for modules and functions at absolute system paths.
    let env = match &z.root {
        None => String::new(),
        Some(r) => {
            let r = r.display();
            format!(
                "module_path=({r}/usr/lib/x86_64-linux-gnu/zsh/5.9)\nfpath=({r}/usr/share/zsh/functions/**/*(/N))\n"
            )
        }
    };
    std::fs::write(dir.join(".zshenv"), &env).unwrap();
    std::fs::write(
        dir.join(".zshrc"),
        format!("{env}PS1='> '\nautoload -U compinit; compinit -u -D\nsource {}\n", file.display()),
    )
    .unwrap();
    let driver = format!(
        "{env}zmodload zsh/zpty\ninteger quiet\nzpty z {zsh} -i\nzpty -w -n z $'{line}\\t'\nout=\n\
         for i in {{1..200}}; do while zpty -r -t z chunk; do out+=$chunk; done; \
         [[ $out == *' -- '* ]] && (( ++quiet > 4 )) && break; sleep 0.05; done\nzpty -d z\nprint -r -- $out\n",
        zsh = zsh.display()
    );
    let run = Command::new(&zsh)
        .args(["-f", "-c", &driver])
        .env("ZDOTDIR", &dir)
        .env("TERM", "xterm")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&run.stdout).replace('\r', "");
    assert!(run.status.success(), "zsh: {}\n{text}", String::from_utf8_lossy(&run.stderr));
    let _ = std::fs::remove_dir_all(&dir);
    {
        strip_ansi(&text)
            .lines()
            .map(str::trim_start)
            .filter(|l| l.starts_with("host.") && l.contains(" -- "))
            .map(|l| l.split_whitespace().next().unwrap().to_string())
            .collect()
    }
}

/// The same Given/When/Then in a real shell that is always present: source
/// the generated bash script, drive it with the COMP_* variables bash sets on
/// TAB, and read COMPREPLY.
#[test]
fn bash_script_sourced_in_real_bash_completes_host_r() {
    let out = mcphost(&["--completions", "bash"]);
    assert_eq!(out.status.code(), Some(0));
    let file = std::env::temp_dir().join(format!("cclib_ac03_{}.bash", std::process::id()));
    std::fs::write(&file, &out.stdout).unwrap();
    let complete = |words: &str, cword: usize| -> Vec<String> {
        let driver = format!(
            "source {}\nCOMP_WORDS=({words}); COMP_CWORD={cword}; COMPREPLY=()\n_mcphost_call\nprintf '%s\\n' \"${{COMPREPLY[@]}}\"\n",
            file.display()
        );
        let run = Command::new("bash").args(["--norc", "-c", &driver]).output().expect("run bash");
        assert!(run.status.success(), "bash: {}", String::from_utf8_lossy(&run.stderr));
        String::from_utf8(run.stdout).unwrap().lines().map(str::to_string).collect()
    };

    let offered = complete("mcphost call host.r", 2);
    println!("bash host.r<TAB> -> {offered:?}");
    assert!(offered.iter().all(|c| c.starts_with("host.r")), "{offered:?}");
    assert!(offered.iter().any(|c| c == "host.redeem"), "{offered:?}");
    assert!(offered.iter().any(|c| c == "host.registry.publish"), "{offered:?}");
    assert!(offered.iter().any(|c| c.starts_with("host.runs.")), "{offered:?}");
    let expected: BTreeSet<String> = call_cli::contract()
        .tools
        .iter()
        .map(|t| t.name.clone())
        .filter(|n| n.starts_with("host.r"))
        .collect();
    assert_eq!(offered.iter().cloned().collect::<BTreeSet<_>>(), expected);

    // Once a tool is chosen, its own flags complete.
    let flags = complete("mcphost call host.tool.publish --k", 3);
    assert!(flags.iter().any(|f| f == "--kind"), "{flags:?}");
}

#[test]
fn bash_and_fish_completions_name_the_same_tools() {
    for shell in ["bash", "fish"] {
        let out = mcphost(&["--completions", shell]);
        assert_eq!(out.status.code(), Some(0), "{shell}");
        let script = String::from_utf8(out.stdout).unwrap();
        for tool in &call_cli::contract().tools {
            assert!(script.contains(&tool.name), "{shell} completions miss {}", tool.name);
        }
    }
}

#[test]
fn list_groups_tools_by_family_with_their_descriptions() {
    let out = mcphost(&["--list"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let runs = lines.iter().position(|l| *l == "host.runs").expect("host.runs family header");
    assert!(lines[runs + 1].starts_with("  host.runs."), "members follow their header: {}", lines[runs + 1]);
    assert!(text.contains("  host.redeem — "), "name — description");
    for tool in &call_cli::contract().tools {
        assert!(text.contains(&format!("  {} — ", tool.name)), "{} missing from --list", tool.name);
    }
}
