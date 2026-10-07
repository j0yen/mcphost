//! PRD-mcphost-registry-listing
//! AC2 (P0) -- Given a committed `registry/server.json` equal to the
//! generator's output, When `--check` runs, Then exit 0; after a
//! description change in source, exit non-zero with a diff.
//!
//! The second half can't mutate `Cargo.toml`'s real `description` field
//! (every other test in this suite binary links against the SAME
//! compiled `env!("CARGO_PKG_DESCRIPTION")`, so changing it for one test
//! would be unobservable without a full rebuild, not a targeted
//! mutation). Instead it writes a committed file that disagrees with a
//! fresh render -- the same shape a stale `description` in `Cargo.toml`
//! would actually produce -- into a scratch path via `--path`, which
//! exercises the identical comparison `--check`'s default path uses.

use std::path::Path;
use std::process::Command;

use crate::common::TempDataDir;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn check_exits_zero_against_the_real_committed_manifest() {
    let bin = env!("CARGO_BIN_EXE_mcphost");
    let output = Command::new(bin)
        .arg("registry-manifest")
        .arg("--check")
        .env_remove("MCPHOST_PUBLIC_URL")
        .current_dir(repo_root())
        .output()
        .expect("spawn mcphost registry-manifest --check");
    assert!(
        output.status.success(),
        "registry/server.json must already equal a fresh render (run `mcphost \
         registry-manifest --write registry/server.json` and commit the result if this \
         fails).\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_exits_nonzero_with_a_diff_when_the_committed_file_has_drifted() {
    let scratch = TempDataDir::new();
    let stale_path = scratch.0.join("server.json");
    // Same shape a stale `description` in Cargo.toml would produce: every
    // other field matches a fresh render, only `description` disagrees.
    std::fs::write(
        &stale_path,
        format!(
            r#"{{
  "$schema": "https://static.modelcontextprotocol.io/schemas/2025-09-29/server.schema.json",
  "description": "a stale description that no longer matches Cargo.toml",
  "license": "MIT OR Apache-2.0",
  "name": "dev.mcphost/mcphost",
  "remotes": [
    {{
      "type": "streamable-http",
      "url": "https://mcphost.dev/mcp"
    }}
  ],
  "repository": {{
    "source": "github",
    "url": "https://github.com/j0yen/mcphost"
  }},
  "version": "{version}",
  "websiteUrl": "https://mcphost.dev"
}}
"#,
            // The real version, not a stale one: this fixture isolates
            // description drift from version drift (--check tolerates
            // the latter; it must still catch the former).
            version = env!("CARGO_PKG_VERSION"),
        ),
    )
    .expect("write stale fixture");

    let bin = env!("CARGO_BIN_EXE_mcphost");
    let output = Command::new(bin)
        .arg("registry-manifest")
        .arg("--check")
        .arg("--path")
        .arg(&stale_path)
        .env_remove("MCPHOST_PUBLIC_URL")
        .output()
        .expect("spawn mcphost registry-manifest --check --path");
    assert!(
        !output.status.success(),
        "a stale committed file must fail --check"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("stale"), "expected a stale notice, got: {stderr}");
    assert!(
        stderr.contains("-") && stderr.contains("+"),
        "expected a +/- diff naming what changed, got: {stderr}"
    );
    assert!(
        stderr.contains("a stale description that no longer matches Cargo.toml"),
        "diff must quote the drifted line, got: {stderr}"
    );
    assert!(
        stderr.contains(env!("CARGO_PKG_DESCRIPTION")),
        "diff must quote the freshly-generated line, got: {stderr}"
    );
}

#[test]
fn check_exits_zero_when_only_the_version_field_has_drifted() {
    let scratch = TempDataDir::new();
    let stale_path = scratch.0.join("server.json");
    // Every field matches a fresh render except `version`: the release
    // land bumps Cargo.toml's version after this file is committed, so
    // --check must tolerate exactly this drift and nothing else.
    std::fs::write(
        &stale_path,
        format!(
            r#"{{
  "$schema": "https://static.modelcontextprotocol.io/schemas/2025-09-29/server.schema.json",
  "description": "{description}",
  "license": "MIT OR Apache-2.0",
  "name": "dev.mcphost/mcphost",
  "remotes": [
    {{
      "type": "streamable-http",
      "url": "https://mcphost.dev/mcp"
    }}
  ],
  "repository": {{
    "source": "github",
    "url": "https://github.com/j0yen/mcphost"
  }},
  "version": "0.0.0-stale",
  "websiteUrl": "https://mcphost.dev"
}}
"#,
            description = env!("CARGO_PKG_DESCRIPTION"),
        ),
    )
    .expect("write version-only-stale fixture");

    let bin = env!("CARGO_BIN_EXE_mcphost");
    let output = Command::new(bin)
        .arg("registry-manifest")
        .arg("--check")
        .arg("--path")
        .arg(&stale_path)
        .env_remove("MCPHOST_PUBLIC_URL")
        .output()
        .expect("spawn mcphost registry-manifest --check --path");
    assert!(
        output.status.success(),
        "a version-only drift must not fail --check (the release land bumps version after \
         this file is committed).\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
