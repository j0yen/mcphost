//! AC6: an `examples/` with directories but zero manifests exits 2 listing
//! every directory; an empty `examples/` exits 0 with `0 examples`.

use std::process::Command;

fn check(dir: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["examples", "check", "--dir"])
        .arg(dir)
        .output()
        .expect("run mcphost examples check")
}

#[test]
fn directories_without_manifests_exit_two_listing_every_one() {
    let tmp = std::env::temp_dir().join(format!("exmf-ac06-a-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    for name in ["alpha", "bravo", "charlie"] {
        std::fs::create_dir_all(tmp.join(name)).unwrap();
    }
    let out = check(&tmp);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    for name in ["alpha", "bravo", "charlie"] {
        assert!(stderr.contains(name), "stderr does not list {name}: {stderr}");
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn an_empty_examples_directory_exits_zero_with_zero_examples() {
    let tmp = std::env::temp_dir().join(format!("exmf-ac06-b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let out = check(&tmp);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("0 examples"));
    let _ = std::fs::remove_dir_all(&tmp);
}
