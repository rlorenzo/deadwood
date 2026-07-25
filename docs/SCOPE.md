# Deadwood — scope and sequencing

## Product direction

A Fallow-style codebase health tool for Rust: point it at a workspace, get a
prioritized, low-noise list of dead and unmaintained code. CLI first; output
formats stable enough (JSON) that IDE/UI layers can be added later without
reworking the core.

## v0.1 — shipped in this baseline

- Workspace discovery via `cargo metadata --no-deps` (all targets: lib, bins,
  tests, examples, benches, build scripts).
- **Dead file detection**: `src/**.rs` unreachable from any target root
  through `mod` resolution (inline mods, `mod.rs`/`name.rs` layouts,
  `#[path]`).
- **Unused pub item detection**: name-census heuristic over the whole
  workspace; conservative (false negatives over false positives); attribute
  and `fn main` escape hatches.
- Text + JSON reporting, CI-friendly exit codes (0 clean / 1 findings /
  2 error).
- Quality gate: fmt + clippy `-D warnings` + tests, locally
  (`scripts/check.sh`) and in CI.

## Next (sequenced, one slice at a time)

1. **Path-aware usage resolution** — resolve `use` paths and qualified calls
   instead of bare name counting; unlocks flagging items that share names and
   detecting unused re-exports (`pub use`).
2. **Unused dependency detection** — compare `Cargo.toml` dependencies
   against `use`/path references per crate.
3. **Config file** (`deadwood.toml`): ignore globs, per-check severity,
   public-API allowlist for library crates.
4. **`cfg` awareness** — evaluate simple `cfg(feature = ...)` / platform
   gates instead of always following them.
5. **Baseline/suppress file** for adopting Deadwood on brownfield codebases.

## Explicitly out of scope for now

- **Duplicate/similar-logic detection** — needs token/AST fingerprinting and
  careful noise control; deferred until the dead-code core is trustworthy.
- **Architecture analysis** (layering, cycles, module coupling metrics).
- **IDE integration, LSP, or any UI/visual reporting** — the JSON output is
  the seam where these will attach later.
- **Plugin system** — no third detector exists yet; premature.
- **Semantic (type-level) analysis** via rustc internals or rust-analyzer —
  revisit once the syntactic approach hits its accuracy ceiling (tracked in
  `docs/ENVIRONMENT.md`).
- **Auto-fix / code removal** — reporting only until precision is proven.

## Design tenets

- Prefer false negatives to false positives; a noisy dead-code tool gets
  uninstalled.
- Every limitation is documented where it lives (module docs) and in the
  README.
- New dependencies only for confirmed problems; std + `syn` + `serde` +
  `clap` + `anyhow` is the current ceiling.
