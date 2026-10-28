# mcphost — coder orientation

## Running tests (agents and interactive sessions)

- Run tests with `cargo nextest run [filter]`. `.config/nextest.toml` terminates any test that runs past 10 min and prints its name with `TIMEOUT`.
- Do not wrap nextest in `timeout`; nextest owns the clock.
- Do not run a bare `cargo test` from an agent session; a hung test then dies nameless.
- Doctests: `timeout 600 cargo test --doc`.
- CI (`.github/workflows/ci.yml`, `registry.yml`) and the build gate run `cargo test` by design. Leave them alone.

Read `CODEMAP.md` (repo root) before you search the tree. It is generated from the
source and lists every `src/**/*.rs` file with a size class and its `//!` summary,
the `// ---- <name> ----` section banners of the big files, and the array registries
(`pub const NAME: &[...]`) that generated contracts hang on.

- Find the owning file in `CODEMAP.md`, then open it. Do not start with `grep -r`/`find`.
- To jump to a section of a big file such as `src/db.rs`:
  `grep -n '^// ---- <name>' src/db.rs`.
- The map has no line numbers on purpose; it only changes when the structure does
  (file added or removed, `//!` summary, banner or registry changed).
- Every `src/**/*.rs` file needs a `//!` module doc in its first 5 lines, or the
  generator refuses to run.
- After adding, removing or renaming a source file, a banner or a registry, run
  `scripts/gen-codemap.sh` and commit the new `CODEMAP.md`. CI's `regen-check` job
  runs `scripts/gen-codemap.sh --check` and goes red on a stale map.
- Never hand-edit `CODEMAP.md`.
