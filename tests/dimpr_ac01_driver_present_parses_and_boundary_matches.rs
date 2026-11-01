//! PRD-mcphost-docs-put-many-live-reproof
//! AC1 -- Given the crate root, When this test runs, Then
//! `tests/live/dimp_ac08_put_many.py` exists, `python3 -I -m py_compile` on it
//! exits 0, and the `BOUNDARY` the test reads by importing the module under
//! `python3 -I` equals `mcphost::docs::DOCS_BOUNDARY` byte for byte.

use std::path::PathBuf;
use std::process::Command;

fn driver() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/live/dimp_ac08_put_many.py")
}

/// Loads the driver by path (module-level code only; the `__main__` guard keeps
/// `main` out, so no network call happens) and writes its `BOUNDARY` as raw bytes.
const READ_BOUNDARY: &str = "import importlib.util, sys\n\
spec = importlib.util.spec_from_file_location('dimp_ac08_put_many', sys.argv[1])\n\
mod = importlib.util.module_from_spec(spec)\n\
spec.loader.exec_module(mod)\n\
sys.stdout.buffer.write(mod.BOUNDARY.encode('utf-8'))\n";

#[test]
fn dimpr_ac01_driver_exists_and_byte_compiles() {
    let path = driver();
    assert!(path.is_file(), "{} must exist", path.display());
    // `-I` ignores PYTHON* env vars, so the .pyc is steered out of tests/live
    // with `-X pycache_prefix`.
    let pycache = std::env::temp_dir().join(format!("mcphost-dimpr-ac01-pyc-{}", std::process::id()));
    let out = Command::new("python3")
        .arg("-I")
        .arg("-X")
        .arg(format!("pycache_prefix={}", pycache.display()))
        .args(["-m", "py_compile"])
        .arg(&path)
        .output()
        .expect("python3 runs");
    std::fs::remove_dir_all(&pycache).ok();
    assert_eq!(out.status.code(), Some(0), "py_compile stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn dimpr_ac01_driver_boundary_equals_docs_boundary_byte_for_byte() {
    let out = Command::new("python3")
        .args(["-I", "-B", "-c", READ_BOUNDARY])
        .arg(driver())
        .output()
        .expect("python3 runs");
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout, mcphost::docs::DOCS_BOUNDARY.as_bytes());
}
