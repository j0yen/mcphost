# Vendored crates

PRD-mcphost-chart-in-a-minute requirement 1 (AC1): these five crates are
vendored from ai-stack's grammar-free chart chain as path dependencies
under `vendor/`. Each keeps its own `Cargo.toml`, `src/`, and test suite,
which runs unmodified as part of this crate's own `cargo test`. The root
`Cargo.toml`'s `[workspace]` table lists each of them as a member, so the
gate's own `cargo test --workspace` / `cargo clippy --workspace` (see
`agent/proof-lanes.toml`, `.buildloop/ci-equivalent.toml`) already covers
them -- no separate CI step needed. (A workspace-less root package would
only make its path dependencies' `#[cfg(test)]` modules reachable via
Cargo's implicit-workspace inference for commands like `cargo test -p
<crate>`; a plain `cargo test`/`cargo test --workspace` still needs the
explicit `members` list to select them for the default run.)

| crate | source repo | commit | role |
|---|---|---|---|
| `mqo-chart-vocab` | `~/projects/ai-stack` | `4ef6c22` | shared `Mark`/`DataType`/`Role` vocabulary the rest of the chain builds on |
| `mqo-result-profiler` | `~/projects/ai-stack` | `4ef6c22` | `result-profile.v1` types only (`ResultProfile`, `ColumnProfile`, `MeasureRange`) -- mcphost writes its own profiler over SQL rows in `src/chart.rs`, not ai-stack's MQO-response profiler |
| `mqo-chart-recommender` | `~/projects/ai-stack` | `4ef6c22` | `recommend(&Value)` -- picks a mark and encoding from a `result-profile.v1` value |
| `mqo-vega-emitter` | `~/projects/ai-stack` | `4ef6c22` | `emit(&recommendation, &rows)` -- an inline-data Vega-Lite v5 spec, with `BigNumber`/`Table` special cases |
| `mqo-chart-caption` | `~/projects/ai-stack` | `4ef6c22` | `generate_caption(&CaptionInput, &CaptionConfig)` -- a headline and up to three facts computed from the rows |

None of the five is modified from its source form beyond what vendoring as
an in-tree path dependency requires (each crate's own `Cargo.toml` names
its sibling vendor crates by relative path instead of a registry version,
since none of the five is published). `deny.toml` (`[sources]
allow-registry`/`allow-git`) and `clippy.toml` apply to them exactly as
they do to `src/`, since they are ordinary path-dependency workspace
members, not vendored via `cargo vendor`'s registry-mirroring mechanism.
